//! # Explorer move protocol
//!
//! Implements the three-step move sequence from MESSAGE_DIAGRAMS.md:
//!
//! 1. `OutgoingExplorerRequest` to the current planet; wait for ack.
//! 2. `IncomingExplorerRequest` (with the explorer's dedicated reply sender)
//!    to the destination planet; wait for ack.
//! 3. `MoveToPlanet` (with the destination planet's explorer sender)
//!    to the explorer; wait for confirmation.
//!
//! TODO(Vivi): `ExplorerHandle` must expose `planet_reply_tx()` returning
//! `Sender<PlanetToExplorer>`. This sender is created at explorer spawn time;
//! the orchestrator holds it and passes it to each new planet on arrival.
//! Replace the `todo!()` in step 2 once that method exists.

use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::utils::ID;
use crossbeam_channel::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs the three-step move protocol synchronously.
///
/// Returns `Ok(())` once the explorer confirms arrival at the destination.
///
/// # Errors
/// - `NotANeighbor` if the destination is not adjacent to the current planet.
/// - `PlanetNotFound` / `ExplorerNotFound` if a registry lookup fails.
/// - `ChannelError` if any step times out or a channel is disconnected.
pub fn execute(
    explorer_id: ID,
    current_planet_id: ID,
    dst_planet_id: ID,
    topology: &Arc<Mutex<Topology>>,
    planets: &Arc<Mutex<PlanetRegistry>>,
    explorers: &Arc<Mutex<ExplorerRegistry>>,
    planet_ack_rx: &Receiver<PlanetToOrchestrator>,
    explorer_ack_rx: &Receiver<crate::explorer::handle::ExplorerToOrchestratorMsg>,
) -> Result<(), OrchestratorError> {
    {
        let topo = topology.lock().unwrap();
        if !topo.are_neighbors(current_planet_id, dst_planet_id) {
            return Err(OrchestratorError::NotANeighbor {
                from: current_planet_id,
                to: dst_planet_id,
            });
        }
    }

    // Step 1: notify the current planet the explorer is leaving.
    {
        let planets = planets.lock().unwrap();
        let current = planets
            .get(current_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(current_planet_id))?;
        current
            .send(OrchestratorToPlanet::OutgoingExplorerRequest { explorer_id })
            .map_err(OrchestratorError::ChannelError)?;
    }
    wait_for_outgoing_ack(planet_ack_rx, current_planet_id)?;

    // Step 2: notify the destination planet the explorer is arriving.
    // The planet needs the explorer's dedicated reply sender so it can
    // communicate back directly.
    // TODO(Vivi): replace todo!() with expl.planet_reply_tx() once that
    // method is added to ExplorerHandle.
    #[allow(clippy::diverging_sub_expression)]
    let planet_reply_tx: crossbeam_channel::Sender<
        common_game::protocols::planet_explorer::PlanetToExplorer,
    > = {
        let _explorers = explorers.lock().unwrap();
        todo!("ExplorerHandle::planet_reply_tx() not yet implemented")
    };

    {
        let planets = planets.lock().unwrap();
        let dst = planets
            .get(dst_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(dst_planet_id))?;
        dst.send(OrchestratorToPlanet::IncomingExplorerRequest {
            explorer_id,
            new_sender: planet_reply_tx,
        })
        .map_err(OrchestratorError::ChannelError)?;
    }
    wait_for_incoming_ack(planet_ack_rx, dst_planet_id)?;

    // Step 3: tell the explorer to switch to the new planet and give it the
    // destination planet's sender so it can write to the planet directly.
    let dst_explorer_tx = {
        let planets = planets.lock().unwrap();
        let dst = planets
            .get(dst_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(dst_planet_id))?;
        dst.explorer_sender()
    };

    {
        let explorers = explorers.lock().unwrap();
        let expl = explorers
            .get(explorer_id)
            .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;
        expl.send(OrchestratorToExplorer::MoveToPlanet {
            sender_to_new_planet: Some(dst_explorer_tx),
            planet_id: dst_planet_id,
        })
        .map_err(OrchestratorError::ChannelError)?;
    }
    wait_for_move_ack(explorer_ack_rx, explorer_id)?;

    {
        let mut explorers = explorers.lock().unwrap();
        if let Some(h) = explorers.get_mut(explorer_id) {
            h.set_current_planet(dst_planet_id);
        }
    }

    log::info!("Explorer {explorer_id} moved from {current_planet_id} to {dst_planet_id}");
    Ok(())
}

fn wait_for_outgoing_ack(
    rx: &Receiver<PlanetToOrchestrator>,
    planet_id: ID,
) -> Result<(), OrchestratorError> {
    match rx.recv_timeout(MOVE_TIMEOUT) {
        Ok(PlanetToOrchestrator::OutgoingExplorerResponse { res, .. }) => {
            res.map_err(OrchestratorError::ChannelError)
        }
        Ok(other) => Err(OrchestratorError::ChannelError(format!(
            "Expected OutgoingExplorerResponse, got {other:?}"
        ))),
        Err(_) => Err(OrchestratorError::ChannelError(format!(
            "Timeout waiting for OutgoingExplorerResponse from planet {planet_id}"
        ))),
    }
}

fn wait_for_incoming_ack(
    rx: &Receiver<PlanetToOrchestrator>,
    planet_id: ID,
) -> Result<(), OrchestratorError> {
    match rx.recv_timeout(MOVE_TIMEOUT) {
        Ok(PlanetToOrchestrator::IncomingExplorerResponse { res, .. }) => {
            res.map_err(OrchestratorError::ChannelError)
        }
        Ok(other) => Err(OrchestratorError::ChannelError(format!(
            "Expected IncomingExplorerResponse, got {other:?}"
        ))),
        Err(_) => Err(OrchestratorError::ChannelError(format!(
            "Timeout waiting for IncomingExplorerResponse from planet {planet_id}"
        ))),
    }
}

/// Waits for the explorer to confirm it has switched planets.
///
/// TODO(Vivi): the explorer must send `MovedToPlanetResult` after handling
/// `MoveToPlanet`. Wire this in explorer_jebediah and explorer_viviana.
fn wait_for_move_ack(
    rx: &Receiver<crate::explorer::handle::ExplorerToOrchestratorMsg>,
    explorer_id: ID,
) -> Result<(), OrchestratorError> {
    use common_game::protocols::orchestrator_explorer::ExplorerToOrchestrator;
    match rx.recv_timeout(MOVE_TIMEOUT) {
        Ok(ExplorerToOrchestrator::MovedToPlanetResult {
            explorer_id: eid,
            planet_id,
        }) if eid == explorer_id => {
            log::debug!("Explorer {explorer_id} confirmed arrival at planet {planet_id}");
            Ok(())
        }
        Ok(other) => Err(OrchestratorError::ChannelError(format!(
            "Expected MovedToPlanetResult for explorer {explorer_id}, got {other:?}"
        ))),
        Err(_) => Err(OrchestratorError::ChannelError(format!(
            "Timeout waiting for MovedToPlanetResult from explorer {explorer_id}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_a_neighbor_is_rejected() {
        let topo = Arc::new(Mutex::new(
            crate::galaxy::parser::parse_str("1 2\n2 1\n3\n").unwrap(),
        ));
        let planets = Arc::new(Mutex::new(PlanetRegistry::new()));
        let explorers = Arc::new(Mutex::new(ExplorerRegistry::new()));
        let (_planet_tx, planet_rx) = crossbeam_channel::unbounded();
        let (_expl_tx, expl_rx) = crossbeam_channel::unbounded::<crate::explorer::handle::ExplorerToOrchestratorMsg>();

        let result = execute(42, 1, 3, &topo, &planets, &explorers, &planet_rx, &expl_rx);

        assert!(
            matches!(result, Err(OrchestratorError::NotANeighbor { from: 1, to: 3 })),
            "expected NotANeighbor, got {result:?}"
        );
    }
}
