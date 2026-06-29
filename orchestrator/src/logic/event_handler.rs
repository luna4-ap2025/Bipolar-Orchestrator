//! # Incoming message handler
//!
//! Drains the `ExplorerToOrchestrator` channel during each logic tick and
//! dispatches messages to the correct subsystem.
//!
//! Messages that arrive here are **autonomous** explorer requests (e.g. the
//! explorer asking to travel to a neighboring planet). Manual/API-triggered
//! messages are handled synchronously in [`crate::api`].
//!
//! ## Owner: Vivi

use crate::explorer::ExplorerRegistry;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use crate::routing;

use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator,
    OrchestratorToExplorer,
};
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;

use crossbeam_channel::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

/// Bag content type used by the orchestrator.
/// Must match the explorer's `ExplorerToOrchestrator<T>`.
pub type BagContent = crate::explorer::handle::BagContent;

/// Drains all pending `ExplorerToOrchestrator` messages without blocking.
///
/// Called once per logic tick so explorers are not left waiting indefinitely.
pub fn drain_explorer_messages(
    explorer_rx: &Receiver<ExplorerToOrchestrator<BagContent>>,
    planet_rx: &Receiver<PlanetToOrchestrator>,
    topology: &Arc<Mutex<Topology>>,
    planets: &Arc<Mutex<PlanetRegistry>>,
    explorers: &Arc<Mutex<ExplorerRegistry>>,
) {
    loop {
        match explorer_rx.try_recv() {
            Ok(msg) => {
                handle_one(
                    msg,
                    explorer_rx,
                    planet_rx,
                    topology,
                    planets,
                    explorers,
                );
            }

            Err(TryRecvError::Empty) => break,

            Err(TryRecvError::Disconnected) => {
                log::error!("Explorer→Orchestrator channel disconnected");
                break;
            }
        }
    }
}

/// Dispatches a single explorer message to the appropriate handler.
fn handle_one(
    msg: ExplorerToOrchestrator<BagContent>,
    explorer_rx: &Receiver<ExplorerToOrchestrator<BagContent>>,
    planet_rx: &Receiver<PlanetToOrchestrator>,
    topology: &Arc<Mutex<Topology>>,
    planets: &Arc<Mutex<PlanetRegistry>>,
    explorers: &Arc<Mutex<ExplorerRegistry>>,
) {
    match msg {
        ExplorerToOrchestrator::NeighborsRequest {
            explorer_id,
            current_planet_id,
        } => {
            log::debug!(
                "Explorer {explorer_id} requests neighbors of planet {current_planet_id}"
            );

            let neighbors = topology
                .lock()
                .unwrap()
                .neighbors(current_planet_id);

            let Ok(explorer_registry) = explorers.lock() else {
                return;
            };

            if let Some(handle) = explorer_registry.get(explorer_id) {
                let _ = handle.send(
                    OrchestratorToExplorer::NeighborsResponse {
                        neighbors,
                    },
                );
            }
        }

        ExplorerToOrchestrator::TravelToPlanetRequest {
            explorer_id,
            current_planet_id,
            dst_planet_id,
        } => {
            log::info!(
                "Explorer {explorer_id} requests travel {current_planet_id} → {dst_planet_id}"
            );

            if let Err(e) = routing::move_explorer::execute(
                explorer_id,
                current_planet_id,
                dst_planet_id,
                topology,
                planets,
                explorers,
                planet_rx,
                explorer_rx,
            ) {
                log::error!(
                    "Failed to move explorer {explorer_id} from {current_planet_id} to {dst_planet_id}: {e}"
                );
            }
        }

        ExplorerToOrchestrator::StartExplorerAIResult { explorer_id } => {
            log::info!("Explorer {explorer_id} AI started");
        }

        ExplorerToOrchestrator::StopExplorerAIResult { explorer_id } => {
            log::info!("Explorer {explorer_id} AI stopped");
        }

        ExplorerToOrchestrator::KillExplorerResult { explorer_id } => {
            log::info!("Explorer {explorer_id} killed");

            if let Ok(mut registry) = explorers.lock() {
                if let Some(mut handle) = registry.remove(explorer_id) {
                    handle.join();
                }
            }
        }

        ExplorerToOrchestrator::MovedToPlanetResult {
            explorer_id,
            planet_id,
        } => {
            log::debug!(
                "Explorer {explorer_id} confirmed arrival at planet {planet_id}"
            );

            if let Ok(mut registry) = explorers.lock() {
                if let Some(handle) = registry.get_mut(explorer_id) {
                    handle.set_current_planet(planet_id);
                }
            }
        }

        other => {
            log::debug!("Logic loop ignoring non-autonomous message: {other:?}");
        }
    }
}