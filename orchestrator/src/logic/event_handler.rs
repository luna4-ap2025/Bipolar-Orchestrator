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
/// Called frequently by the logic loop so explorers are not left waiting.
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

            // We still check that the explorer exists.
            // But IMPORTANT:
            // NeighborsResponse does not contain the planet id it refers to.
            // Therefore, if the request says "planet 5", we must answer with
            // neighbors of planet 5, not with the registry's current planet.
            let real_current_planet = {
                let Ok(registry) = explorers.lock() else {
                    log::warn!(
                        "Could not lock explorer registry while handling NeighborsRequest from explorer {explorer_id}"
                    );
                    return;
                };

                let Some(handle) = registry.get(explorer_id) else {
                    log::warn!(
                        "Ignoring NeighborsRequest from unknown explorer {explorer_id}"
                    );
                    return;
                };

                handle.current_planet()
            };

            if real_current_planet != current_planet_id {
                log::warn!(
                    "NeighborsRequest from explorer {explorer_id} used planet {current_planet_id}, while registry says {real_current_planet}. Responding with requested planet to keep explorer mapping consistent."
                );
            }

            let neighbors = {
                let Ok(topo) = topology.lock() else {
                    log::warn!(
                        "Could not lock topology while handling NeighborsRequest from explorer {explorer_id}"
                    );
                    return;
                };

                topo.neighbors(current_planet_id)
                    .into_iter()
                    .filter(|neighbor_id| *neighbor_id != current_planet_id)
                    .collect::<Vec<_>>()
            };

            let Ok(explorer_registry) = explorers.lock() else {
                log::warn!(
                    "Could not lock explorer registry to send NeighborsResponse to explorer {explorer_id}"
                );
                return;
            };

            if let Some(handle) = explorer_registry.get(explorer_id) {
                if let Err(e) = handle.send(
                    OrchestratorToExplorer::NeighborsResponse {
                        neighbors,
                    },
                ) {
                    log::error!(
                        "Failed sending NeighborsResponse to explorer {explorer_id}: {e}"
                    );
                }
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

            let real_current_planet = {
                let Ok(registry) = explorers.lock() else {
                    log::warn!(
                        "Could not lock explorer registry while handling TravelToPlanetRequest from explorer {explorer_id}"
                    );
                    return;
                };

                let Some(handle) = registry.get(explorer_id) else {
                    log::warn!(
                        "Ignoring TravelToPlanetRequest from unknown explorer {explorer_id}"
                    );
                    return;
                };

                handle.current_planet()
            };

            // For travel requests, unlike NeighborsRequest, the registry must win.
            // If the explorer is asking to travel from a planet that the registry
            // does not consider current anymore, the request is stale and unsafe.
            if real_current_planet != current_planet_id {
                log::warn!(
                    "Ignoring stale TravelToPlanetRequest from explorer {explorer_id}: requested from {current_planet_id}, but registry says {real_current_planet}"
                );
                return;
            }

            if dst_planet_id == current_planet_id {
                log::warn!(
                    "Ignoring TravelToPlanetRequest from explorer {explorer_id}: destination equals current planet {current_planet_id}"
                );
                return;
            }

            if let Err(e) = routing::move_explorer::execute(
                explorer_id,
                current_planet_id,
                dst_planet_id,
                topology,
                planets,
                explorers,
                planet_rx,
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

        ExplorerToOrchestrator::ResetExplorerAIResult { explorer_id } => {
            log::info!("Explorer {explorer_id} AI reset");
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

            // Optional safety check: only accept the reported planet if it still
            // exists in topology. This avoids updating the registry to a planet
            // that was destroyed meanwhile.
            let planet_still_exists = {
                let Ok(topo) = topology.lock() else {
                    log::warn!(
                        "Could not lock topology while handling MovedToPlanetResult from explorer {explorer_id}"
                    );
                    return;
                };

                topo.planet_ids().any(|id| id == planet_id)
            };

            if !planet_still_exists {
                log::warn!(
                    "Ignoring MovedToPlanetResult from explorer {explorer_id}: planet {planet_id} no longer exists"
                );
                return;
            }

            if let Ok(mut registry) = explorers.lock() {
                if let Some(handle) = registry.get_mut(explorer_id) {
                    handle.set_current_planet(planet_id);

                    log::info!(
                        "Explorer {explorer_id} registry position updated to planet {planet_id}"
                    );
                } else {
                    log::warn!(
                        "Received MovedToPlanetResult from unknown explorer {explorer_id}"
                    );
                }
            }
        }

        ExplorerToOrchestrator::CurrentPlanetResult {
            explorer_id,
            planet_id,
        } => {
            log::debug!(
                "Explorer {explorer_id} reports current planet {planet_id}"
            );

            let planet_still_exists = {
                let Ok(topo) = topology.lock() else {
                    log::warn!(
                        "Could not lock topology while handling CurrentPlanetResult from explorer {explorer_id}"
                    );
                    return;
                };

                topo.planet_ids().any(|id| id == planet_id)
            };

            if !planet_still_exists {
                log::warn!(
                    "Ignoring CurrentPlanetResult from explorer {explorer_id}: planet {planet_id} no longer exists"
                );
                return;
            }

            if let Ok(mut registry) = explorers.lock() {
                if let Some(handle) = registry.get_mut(explorer_id) {
                    handle.set_current_planet(planet_id);
                }
            }
        }

        ExplorerToOrchestrator::GenerateResourceResponse {
            explorer_id,
            generated,
        } => {
            log::debug!(
                "Explorer {explorer_id} generated resource result: {generated:?}"
            );
        }

        ExplorerToOrchestrator::CombineResourceResponse {
            explorer_id,
            generated,
        } => {
            log::debug!(
                "Explorer {explorer_id} combined resource result: {generated:?}"
            );
        }

        ExplorerToOrchestrator::SupportedResourceResult {
            explorer_id,
            supported_resources,
        } => {
            log::debug!(
                "Explorer {explorer_id} supported resources: {supported_resources:?}"
            );
        }

        ExplorerToOrchestrator::SupportedCombinationResult {
            explorer_id,
            combination_list,
        } => {
            log::debug!(
                "Explorer {explorer_id} supported combinations: {combination_list:?}"
            );
        }

        ExplorerToOrchestrator::BagContentResponse {
            explorer_id,
            bag_content,
        } => {
            log::debug!(
                "Explorer {explorer_id} bag content: {bag_content:?}"
            );
        }
    }
}