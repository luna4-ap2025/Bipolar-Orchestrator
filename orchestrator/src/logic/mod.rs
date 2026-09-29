//! The logic loop, running in its own thread:
//! - every 0.7s it handles the messages explorers send on their own
//!   (neighbors, travel, move confirmations...)
//! - every 25s it does a planet tick: each planet gets a sunray or an
//!   asteroid depending on its curve, and a planet with no rocket is destroyed
//!   (see `tick.rs`)
//!
//! It stops when asked to, or when all the explorers are dead.

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

// was 4s, but planets rolled so often that games were over way too fast
const TICK_INTERVAL: Duration = Duration::from_secs(25);

// Has to be much shorter than TICK_INTERVAL: if explorers only got an answer
// every 25s they'd time out and retry.
const MESSAGE_POLL_INTERVAL: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy)]
pub(super) enum ControlSignal {
    Stop,
}

pub struct LogicController {
    // Some while running
    control_tx: Option<Sender<ControlSignal>>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl LogicController {
    #[must_use]
    pub fn new() -> Self {
        Self {
            control_tx: None,
            thread_handle: None,
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.control_tx.is_some()
    }

    /// Starts the logic loop thread.
    ///
    /// # Errors
    /// `InvalidState` if it's already running.
    // same arguments as the fields of OrchestratorApi, so there are many
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

    /// Stops the loop and waits for the thread to end.
    ///
    /// # Errors
    /// `InvalidState` if it isn't running.
    pub fn stop(&mut self) -> Result<(), OrchestratorError> {
        let tx = self
            .control_tx
            .take()
            .ok_or_else(|| OrchestratorError::InvalidState("Logic is not running".to_string()))?;

        // can fail if the thread already ended (game over), that's fine
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

// The Arcs are taken by value because they're moved into the thread.
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

    // first tick right away
    let mut next_tick_at = Instant::now();

    loop {
        match control_rx.try_recv() {
            Ok(ControlSignal::Stop) | Err(TryRecvError::Disconnected) => {
                log::info!("Logic loop: stop signal received");
                break;
            }

            Err(TryRecvError::Empty) => {}
        }

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

        // game over
        if explorers.lock().unwrap().is_empty() {
            log::info!("All explorers have died - game over, logic loop exiting");
            break;
        }

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

        // again, a planet dying can kill the last explorer
        if explorers.lock().unwrap().is_empty() {
            log::info!("All explorers have died - game over, logic loop exiting");
            break;
        }

        std::thread::sleep(MESSAGE_POLL_INTERVAL);
    }
}

fn run_planet_tick(
    topology: &Arc<Mutex<Topology>>,
    planets: &Arc<Mutex<PlanetRegistry>>,
    explorers: &Arc<Mutex<ExplorerRegistry>>,
    prob_registry: &Arc<Mutex<ProbabilityRegistry>>,
    planet_rx: &Arc<Mutex<Receiver<PlanetToOrchestrator>>>,
    forge: &Arc<Mutex<Forge>>,
    rng: &mut impl rand::Rng,
) {
    let delta = TICK_INTERVAL.as_secs_f64();

    if let Ok(mut prob) = prob_registry.lock() {
        prob.tick(delta);
    }

    let planet_ids: Vec<_> = {
        let topo = topology.lock().unwrap();
        topo.planet_ids().collect()
    };

    if planet_ids.is_empty() {
        log::info!("All planets destroyed");
        return;
    }

    // at most one planet dies per tick, so the galaxy doesn't collapse at once
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
