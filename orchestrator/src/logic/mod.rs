//! # Game logic controller
//!
//! The **logic loop** is the autonomous heartbeat of the game. Once started it
//! runs in a dedicated background thread and:
//!
//! 1. **Ticks** a configurable interval (default: 1 second).
//! 2. For each alive planet, rolls a die against its probability curve and sends
//!    either a [`Sunray`] or an [`Asteroid`].
//! 3. Waits for the planet's acknowledgment.
//! 4. If an [`AsteroidAck`] carries `rocket: None`, the planet is destroyed:
//!    - Sends [`KillPlanet`] and removes it from all registries.
//!    - Notifies the [`ProbabilityRegistry`] to trigger the bipolar flip.
//!    - Notifies the [`Topology`] to remove the dead planet.
//! 5. Can be stopped and restarted at any time via [`LogicController::stop`] /
//!    [`LogicController::start`].
//!
//! ## Owner: Vivi

pub mod event_handler;
pub mod tick;

use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use crate::probability::ProbabilityRegistry;
use common_game::components::forge::Forge;
use crate::explorer::handle::ExplorerToOrchestratorMsg;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the logic loop ticks.
///
/// History: 1s -> 4s (planets died in seconds). Then at 4s, real playtests
/// still wiped the whole galaxy in ~24s of actual dispatching — moderate,
/// ordinary (non-worst-case) per-tick risk in the 40-60% range, rolled
/// independently across 7 planets every 4s, compounds into total wipeout
/// almost immediately regardless of how safe any single roll looks. Slowing
/// hostility and flooring the curves controls *how dangerous* a roll is;
/// this controls *how often* a roll happens at all — raised to 25s, targeting
/// a full game lasting roughly 10 minutes. `curves::PERIOD` and
/// `probability::HOSTILITY_PER_SEC` were scaled to match.
const TICK_INTERVAL: Duration = Duration::from_secs(25);

/// Signal sent to the logic thread to control it.
#[derive(Debug, Clone, Copy)]
pub(super) enum ControlSignal {
    Stop,
}

/// Controls the autonomous game logic thread.
///
/// [`LogicController::start`] launches a background thread. [`LogicController::stop`]
/// sends a stop signal and waits for it to finish.
pub struct LogicController {
    /// Channel end for sending stop signals to the logic thread.
    control_tx: Option<Sender<ControlSignal>>,
    /// Join handle for the background thread.
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl LogicController {
    /// Creates a new (stopped) controller.
    pub fn new() -> Self {
        Self {
            control_tx: None,
            thread_handle: None,
        }
    }

    /// Returns `true` if the logic loop is currently running.
    pub fn is_running(&self) -> bool {
        self.control_tx.is_some()
    }

    /// Starts the logic loop in a background thread.
    ///
    /// The loop has access to the shared game state via `Arc<Mutex<...>>` wrappers.
    ///
    /// # Errors
    /// Returns [`OrchestratorError::InvalidState`] if already running.
    pub fn start(
        &mut self,
        topology: Arc<Mutex<Topology>>,
        planets: Arc<Mutex<PlanetRegistry>>,
        explorers: Arc<Mutex<ExplorerRegistry>>,
        prob_registry: Arc<Mutex<ProbabilityRegistry>>,
        planet_rx: Arc<Mutex<Receiver<PlanetToOrchestrator>>>,
        explorer_rx: Arc<Mutex<Receiver<ExplorerToOrchestratorMsg>>>,
        forge: Arc<Mutex<Forge>>,
    ) -> Result<(), OrchestratorError> {
        if self.is_running() {
            return Err(OrchestratorError::InvalidState(
                "Logic is already running".to_string(),
            ));
        }

        let (control_tx, control_rx) = bounded::<ControlSignal>(1);

        let handle = std::thread::Builder::new()
            .name("bipolar-logic".to_string())
            .spawn(move || {
                run_logic_loop(
                    topology,
                    planets,
                    explorers,
                    prob_registry,
                    planet_rx,
                    explorer_rx,
                    forge,
                    control_rx,
                );
            })
            .map_err(|e| OrchestratorError::InvalidState(e.to_string()))?;

        self.control_tx = Some(control_tx);
        self.thread_handle = Some(handle);
        log::info!("Logic loop started");
        Ok(())
    }

    /// Stops the logic loop and waits for the thread to finish.
    ///
    /// # Errors
    /// Returns [`OrchestratorError::InvalidState`] if not running.
    pub fn stop(&mut self) -> Result<(), OrchestratorError> {
        let tx = self.control_tx.take().ok_or_else(|| {
            OrchestratorError::InvalidState("Logic is not running".to_string())
        })?;

        // send stop signal (ok if it fails, thread may have exited already)
        let _ = tx.send(ControlSignal::Stop);

        if let Some(handle) = self.thread_handle.take() {
            if let Err(e) = handle.join() {
                log::error!("Logic thread panicked: {e:?}");
            }
        }

        log::info!("Logic loop stopped");
        Ok(())
    }
}

impl Default for LogicController {
    fn default() -> Self {
        Self::new()
    }
}

/// The main body of the logic loop thread.
///
/// Runs until a [`ControlSignal::Stop`] is received or all planets are dead.
fn run_logic_loop(
    topology: Arc<Mutex<Topology>>,
    planets: Arc<Mutex<PlanetRegistry>>,
    explorers: Arc<Mutex<ExplorerRegistry>>,
    prob_registry: Arc<Mutex<ProbabilityRegistry>>,
    planet_rx: Arc<Mutex<Receiver<PlanetToOrchestrator>>>,
    explorer_rx: Arc<Mutex<Receiver<ExplorerToOrchestratorMsg>>>,
    forge: Arc<Mutex<Forge>>,
    control_rx: Receiver<ControlSignal>,
) {
    let mut rng = rand::rng();

    loop {
        // check if we got a stop signal before doing anything this tick
        match control_rx.try_recv() {
            Ok(ControlSignal::Stop) | Err(TryRecvError::Disconnected) => {
                log::info!("Logic loop: stop signal received");
                break;
            }
            Err(TryRecvError::Empty) => {}
        }

        // advance the probability curves by one tick worth of time
        let delta = TICK_INTERVAL.as_secs_f64();
        if let Ok(mut prob) = prob_registry.lock() {
            prob.tick(delta);
        }

        // snapshot the alive planet ids while holding the lock, then release it
        // so dispatch_to_planet can re-acquire it per planet
        let planet_ids: Vec<_> = {
            let topo = topology.lock().unwrap();
            topo.planet_ids().collect()
        };

        if planet_ids.is_empty() {
            log::info!("All planets destroyed - logic loop exiting");
            break;
        }

        // For each alive planet, roll the dice and send sunray or asteroid —
        // but stop as soon as one planet dies this tick. Dispatching every
        // planet unconditionally every tick let 3+ planets die in the same
        // instant (confirmed in a real playtest log), which also crushed
        // hostility: `on_planet_death`'s dampening applied 2-3 times back to
        // back within the same tick, before hostility ever had a chance to
        // climb, which is why Eclipse mode felt unreachable even after
        // hostility stopped hard-resetting to zero.
        for planet_id in planet_ids {
            let mut prob = prob_registry.lock().unwrap();
            let mut planets_guard = planets.lock().unwrap();
            let mut topology_guard = topology.lock().unwrap();
            let planet_rx_guard = planet_rx.lock().unwrap();
            let mut explorers_guard = explorers.lock().unwrap();

            let forge_guard = forge.lock().unwrap();

            match tick::dispatch_to_planet(
                planet_id,
                &forge_guard,
                &mut prob,
                &mut planets_guard,
                &mut topology_guard,
                &planet_rx_guard,
                &mut explorers_guard,
                &mut rng,
            ) {
                Ok(destroyed) => {
                    if destroyed {
                        break;
                    }
                }
                Err(e) => log::error!("Tick dispatch failed for planet {planet_id}: {e}"),
            }
        }

        // Game's end condition (spec §1.3): stop once every explorer has died.
        // Checked every tick since an explorer can die mid-tick when their
        // planet is destroyed in the loop above.
        if explorers.lock().unwrap().is_empty() {
            log::info!("All explorers have died - game over, logic loop exiting");
            break;
        }

        // drain any messages from explorers that arrived autonomously this tick
        {
            let explorer_rx_guard = explorer_rx.lock().unwrap();
            let planet_rx_guard = planet_rx.lock().unwrap();

            event_handler::drain_explorer_messages(
                &explorer_rx_guard,
                &planet_rx_guard,
                &topology,
                &planets,
                &explorers,
            );
        }
        std::thread::sleep(TICK_INTERVAL);
    }
}
