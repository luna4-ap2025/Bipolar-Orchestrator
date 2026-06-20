//! # Explorer move protocol
//!
//! Implements the full three-step move sequence.
//!
//! ## Owner: Vale

use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, unbounded};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Executes the three-step explorer move protocol synchronously.
///
/// Returns `Ok(())` when the explorer has acknowledged its arrival at the new planet.
///
/// # Errors
/// - [`OrchestratorError::NotANeighbor`] if `dst` is not adjacent to `current`.
/// - [`OrchestratorError::PlanetNotFound`] if either planet doesn't exist.
/// - [`OrchestratorError::ExplorerNotFound`] if the explorer doesn't exist.
/// - [`OrchestratorError::ChannelError`] if any step times out or disconnects.
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
    // first check that the move is actually valid
    {
        let topo = topology.lock().unwrap();
        if !topo.are_neighbors(current_planet_id, dst_planet_id) {
            return Err(OrchestratorError::NotANeighbor {
                from: current_planet_id,
                to: dst_planet_id,
            });
        }
    }

    // step 1: tell the current planet the explorer is leaving so it drops their sender
    {
        let planets = planets.lock().unwrap();
        let current = planets
            .get(current_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(current_planet_id))?;
        current
            .send(OrchestratorToPlanet::OutgoingExplorerRequest { explorer_id })
            .map_err(OrchestratorError::ChannelError)?;
    }

    wait_for_outgoing_ack(planet_ack_rx, explorer_id, current_planet_id)?;

    // step 2: tell the destination planet the explorer is arriving
    // we give it a sender so it can reply directly to this explorer
    // NOTE: the explorer needs a dedicated PlanetToExplorer channel per visit
    // TODO(Vale): decide whether to create a fresh channel pair here or reuse
    // a per-explorer channel created at spawn time (see OWNERSHIP.md for context)
    let (tx_expl_to_dst, rx_expl_to_dst) = unbounded::<ExplorerToPlanet>();

    let dst_explorer_sender = {
        let planets = planets.lock().unwrap();
        let dst = planets
            .get(dst_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(dst_planet_id))?;
        dst.explorer_sender()
    };

    {
        let planets = planets.lock().unwrap();
        let dst = planets
            .get(dst_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(dst_planet_id))?;
        dst.send(OrchestratorToPlanet::IncomingExplorerRequest {
            explorer_id,
            new_sender: {
                // TODO(Vale): pass the correct Sender<PlanetToExplorer> here
                // the planet needs this to send responses back to the explorer
                todo!("pass the correct planet->explorer Sender here")
            },
        })
        .map_err(OrchestratorError::ChannelError)?;
    }

    wait_for_incoming_ack(planet_ack_rx, explorer_id, dst_planet_id)?;

    // step 3: tell the explorer to switch planets, give it the new planet's sender
    {
        let explorers = explorers.lock().unwrap();
        let expl = explorers
            .get(explorer_id)
            .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;
        expl.send(OrchestratorToExplorer::MoveToPlanet {
            sender_to_new_planet: Some(dst_explorer_sender),
            planet_id: dst_planet_id,
        })
        .map_err(OrchestratorError::ChannelError)?;
    }

    wait_for_move_ack(explorer_ack_rx, explorer_id)?;

    // update our local record of where the explorer is
    {
        let mut explorers = explorers.lock().unwrap();
        if let Some(h) = explorers.get_mut(explorer_id) {
            h.set_current_planet(dst_planet_id);
        }
    }

    log::info!("Explorer {explorer_id} moved {current_planet_id} → {dst_planet_id}");
    Ok(())
}

fn wait_for_outgoing_ack(
    rx: &Receiver<PlanetToOrchestrator>,
    explorer_id: ID,
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
    explorer_id: ID,
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

fn wait_for_move_ack(
    rx: &Receiver<crate::explorer::handle::ExplorerToOrchestratorMsg>,
    explorer_id: ID,
) -> Result<(), OrchestratorError> {
    // TODO(Vale): receive MovedToPlanetResult and verify explorer_id matches
    // match rx.recv_timeout(MOVE_TIMEOUT) {
    //     Ok(ExplorerToOrchestrator::MovedToPlanetResult { explorer_id: eid, .. }) if eid == explorer_id => Ok(()),
    //     ...
    // }
    Ok(()) // placeholder
}
