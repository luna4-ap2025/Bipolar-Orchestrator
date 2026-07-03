//! # Game logic controller
//!
//! The **logic loop** is the autonomous heartbeat of the game. Once started it
//! runs in a dedicated background thread and:
//!
//! 1. Continuously drains autonomous explorer messages at a short polling interval.
//! 2. Every [`TICK_INTERVAL`], advances probability and dispatches one game tick.
//! 3. For each alive planet, rolls a die against its probability curve and sends
//!    either a [`Sunray`] or an [`Asteroid`].
//! 4. Waits for the planet's acknowledgment.
//! 5. If an [`AsteroidAck`] carries `rocket: None`, the planet is destroyed:
//!    - Sends [`KillPlanet`] and removes it from all registries.
//!    - Notifies the [`ProbabilityRegistry`] to trigger the bipolar flip.
//!    - Notifies the [`Topology`] to remove the dead planet.
//! 6. Can be stopped and restarted at any time via [`LogicController::stop`] /
//!    [`LogicController::start`].
//!
//! ## Owner: Vivi

pub mod event_handler;
pub mod tick;

use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::explorer::handle::ExplorerToOrchestratorMsg;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use crate::probability::ProbabilityRegistry;

use common_game::components::forge::Forge;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the probability/asteroid/sunray game tick runs.
const TICK_INTERVAL: Duration = Duration::from_secs(25);

/// How often the logic loop checks autonomous explorer messages.
///
/// This must be much shorter than `TICK_INTERVAL`: explorers may ask for
/// neighbors, travel, or send move confirmations at any time. If we only read
/// their channel once every 25 seconds, explorers time out and retry even
/// though the orchestrator is alive.
const MESSAGE_POLL_INTERVAL: Duration = Duration::from_millis(700);

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
    /// Creates a new stopped controller.
    #[must_use]
    pub fn new() -> Self {
        Self {
            control_tx: None,
            thread_handle: None,
        }
    }

    /// Returns `true` if the logic loop is currently running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.control_tx.is_some()
    }

    /// Starts the logic loop in a background thread.
    ///
    /// The loop has access to the shared game state via `Arc<Mutex<...>>` wrappers.
    ///
    /// # Errors
    /// Returns [`OrchestratorError::InvalidState`] if already running.
    ///
    /// Takes each shared registry as its own `Arc<Mutex<_>>` parameter (rather
    /// than bundling them into a struct) to mirror `OrchestratorApi`'s own
    /// field layout one-to-one; this is a deliberate, accepted tradeoff
    /// against `clippy::pedantic`'s argument-count threshold.
    #[allow(clippy::too_many_arguments)]
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
        let tx = self
            .control_tx
            .take()
            .ok_or_else(|| OrchestratorError::InvalidState("Logic is not running".to_string()))?;

        // Send stop signal. It is fine if it fails: the thread may have exited already.
        let _ = tx.send(ControlSignal::Stop);

        if let Some(handle) = self.thread_handle.take()
            && let Err(e) = handle.join()
        {
            log::error!("Logic thread panicked: {e:?}");
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
/// Runs until a [`ControlSignal::Stop`] is received, all planets are destroyed,
/// or all explorers are dead.
///
/// Important behavior:
/// - explorer messages are drained every [`MESSAGE_POLL_INTERVAL`];
/// - probability/asteroid/sunray ticks run only every [`TICK_INTERVAL`].
///
/// Takes owned `Arc<Mutex<_>>` clones (not references) by design: this
/// function is spawned into its own thread, which requires `'static` owned
/// handles to move into the closure — `clippy::pedantic`'s
/// `needless_pass_by_value` doesn't account for that requirement. It also
/// mirrors `OrchestratorApi`'s field layout one-to-one, hence the argument
/// count.
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
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

    // Run the first planet tick immediately after startup, then every 25 seconds.
    let mut next_tick_at = Instant::now();

    loop {
        // Check if we got a stop signal.
        match control_rx.try_recv() {
            Ok(ControlSignal::Stop) | Err(TryRecvError::Disconnected) => {
                log::info!("Logic loop: stop signal received");
                break;
            }

            Err(TryRecvError::Empty) => {}
        }

        // Drain autonomous explorer messages frequently.
        {
            let explorer_rx_guard = explorer_rx.lock().unwrap();
            let planet_rx_guard = planet_rx.lock().unwrap();

            event_handler::drain_explorer_messages(
                &explorer_rx_guard,
                &planet_rx_guard,
                &topology,
                &planets,
                &explorers,
                &prob_registry,
            );
        }

        // Game's end condition: stop once every explorer has died.
        if explorers.lock().unwrap().is_empty() {
            log::info!("All explorers have died - game over, logic loop exiting");
            break;
        }

        // Run probability/planet tick only when its interval expires.
        if Instant::now() >= next_tick_at {
            run_planet_tick(
                &topology,
                &planets,
                &explorers,
                &prob_registry,
                &planet_rx,
                &forge,
                &mut rng,
            );

            next_tick_at = Instant::now() + TICK_INTERVAL;
        }

        // Check again after the planet tick, because a planet death may have
        // killed the last explorer.
        if explorers.lock().unwrap().is_empty() {
            log::info!("All explorers have died - game over, logic loop exiting");
            break;
        }

        std::thread::sleep(MESSAGE_POLL_INTERVAL);
    }
}

/// Runs one probability/asteroid/sunray tick over the currently alive planets.
fn run_planet_tick(
    topology: &Arc<Mutex<Topology>>,
    planets: &Arc<Mutex<PlanetRegistry>>,
    explorers: &Arc<Mutex<ExplorerRegistry>>,
    prob_registry: &Arc<Mutex<ProbabilityRegistry>>,
    planet_rx: &Arc<Mutex<Receiver<PlanetToOrchestrator>>>,
    forge: &Arc<Mutex<Forge>>,
    rng: &mut impl rand::Rng,
) {
    // Advance the probability curves by one tick worth of time.
    let delta = TICK_INTERVAL.as_secs_f64();

    if let Ok(mut prob) = prob_registry.lock() {
        prob.tick(delta);
    }

    // Snapshot alive planet IDs while holding the topology lock, then release it.
    let planet_ids: Vec<_> = {
        let topo = topology.lock().unwrap();
        topo.planet_ids().collect()
    };

    if planet_ids.is_empty() {
        log::info!("All planets destroyed");
        return;
    }

    // For each alive planet, roll the dice and send sunray or asteroid.
    // Stop as soon as one planet dies this tick.
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
            rng,
        ) {
            Ok(destroyed) => {
                if destroyed {
                    break;
                }
            }

            Err(e) => {
                log::error!("Tick dispatch failed for planet {planet_id}: {e}");
            }
        }
    }
}
