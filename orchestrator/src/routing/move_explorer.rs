//! # Explorer move protocol
//!
//! Implements the full three-step move sequence described in the project spec
//! and in `MESSAGE_DIAGRAMS.md` ("Moving to another planet").
//!
//! ## Steps
//! 1. `OutgoingExplorerRequest` → current planet → wait for `OutgoingExplorerResponse`.
//! 2. `IncomingExplorerRequest` → destination planet (with the explorer's dedicated
//!    `Sender<PlanetToExplorer>`) → wait for `IncomingExplorerResponse`.
//! 3. `MoveToPlanet` → explorer (with the destination planet's `ExplorerToPlanet` sender)
//!    → wait for `MovedToPlanetResult`.
//!
//! ## Owner: Vale
//!
//! ## Vivi dependency
//! [`crate::explorer::handle::ExplorerHandle`] must expose a
//! `planet_reply_tx(&self) -> Sender<PlanetToExplorer>` method (or a public field)
//! that returns the dedicated sender the planet uses to reply to this explorer.
//! The orchestrator creates this channel at explorer spawn time and keeps the sender;
//! the explorer keeps the receiver permanently (it never changes across planet moves).

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

/// Executes the three-step explorer move protocol synchronously.
///
/// Returns `Ok(())` when the orchestrator has sent `MoveToPlanet` to the explorer
/// and received `MovedToPlanetResult` back.
///
/// # Errors
/// - [`OrchestratorError::NotANeighbor`] if `dst` is not adjacent to `current`.
/// - [`OrchestratorError::PlanetNotFound`] if either planet doesn't exist.
/// - [`OrchestratorError::ExplorerNotFound`] if the explorer doesn't exist.
/// - [`OrchestratorError::ChannelError`] if any step times out or disconnects.
///
/// # Vivi dependency
/// Requires `ExplorerHandle::planet_reply_tx()` to exist — see module-level docs.
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
    // Validate the move is topologically legal.
    {
        let topo = topology.lock().unwrap();
        if !topo.are_neighbors(current_planet_id, dst_planet_id) {
            return Err(OrchestratorError::NotANeighbor {
                from: current_planet_id,
                to: dst_planet_id,
            });
        }
    }

    // ── Step 1 ────────────────────────────────────────────────────────────────
    // Tell the CURRENT planet the explorer is leaving.
    // The planet drops the explorer's Sender<PlanetToExplorer> from its map.
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

    // ── Step 2 ────────────────────────────────────────────────────────────────
    // Tell the DESTINATION planet the explorer is arriving.
    // We pass the explorer's dedicated Sender<PlanetToExplorer> so the planet
    // can reply directly to this explorer (the explorer's rx_planet never changes).
    //
    // VIVI: ExplorerHandle must have `planet_reply_tx(&self) -> Sender<PlanetToExplorer>`.
    // The Sender is created at explorer spawn time (orchestrator keeps sender,
    // explorer keeps receiver).
    // VIVI: replace `todo!()` with `expl.planet_reply_tx()` once ExplorerHandle
    // has that method (see module-level doc for the exact field/method to add).
    #[allow(clippy::diverging_sub_expression)]
    let planet_reply_tx = {
        let explorers = explorers.lock().unwrap();

        let explorer = explorers
            .get(explorer_id)
            .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;

        explorer.planet_reply_tx()
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

    // ── Step 3 ────────────────────────────────────────────────────────────────
    // Tell the EXPLORER to switch to the new planet.
    // We give it the destination planet's ExplorerToPlanet sender so it can
    // write to the new planet (the explorer replaces its tx_planet with this).
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

    // Update the orchestrator's record of the explorer's location.
    {
        let mut explorers = explorers.lock().unwrap();
        if let Some(h) = explorers.get_mut(explorer_id) {
            h.set_current_planet(dst_planet_id);
        }
    }

    log::info!("Explorer {explorer_id} moved {current_planet_id} → {dst_planet_id}");
    Ok(())
}

// ── Private helpers ────────────────────────────────────────────────────────────

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

/// Waits for the explorer to confirm it has switched to the new planet.
///
/// # Vivi dependency
/// The explorer must send `MovedToPlanetResult` after handling `MoveToPlanet`.
/// Currently the explorer does NOT send this — Vivi must add it.
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
        // Build topology via the public parser: 1↔2, 3 is isolated
        let topo = Arc::new(Mutex::new(
            crate::galaxy::parser::parse_str("1 2\n2 1\n3\n").unwrap(),
        ));
        let planets = Arc::new(Mutex::new(PlanetRegistry::new()));
        let explorers = Arc::new(Mutex::new(ExplorerRegistry::new()));

        let (_planet_tx, planet_rx) = crossbeam_channel::unbounded();
        let (_expl_tx, expl_rx) = crossbeam_channel::unbounded::<crate::explorer::handle::ExplorerToOrchestratorMsg>();

        let result = execute(
            42,   // explorer_id
            1,    // current_planet_id
            3,    // dst_planet_id — NOT a neighbor of 1
            &topo,
            &planets,
            &explorers,
            &planet_rx,
            &expl_rx,
        );

        assert!(
            matches!(result, Err(OrchestratorError::NotANeighbor { from: 1, to: 3 })),
            "expected NotANeighbor error, got {result:?}"
        );
    }
}
