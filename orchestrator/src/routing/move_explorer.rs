//! Moving an explorer, following "Moving to another planet" in the common
//! crate's `MESSAGE_DIAGRAMS.md`:
//! 1. `OutgoingExplorerRequest` to the current planet, wait for the response
//! 2. `IncomingExplorerRequest` to the destination (with the explorer's
//!    sender so the planet can answer it), wait for the response
//! 3. `MoveToPlanet` to the explorer, with the destination planet's sender
//!
//! We don't wait for `MovedToPlanetResult` here. All explorers share one
//! channel, so the other explorer's messages can arrive first. The event
//! handler deals with it instead.

use crate::ack::recv_ack;
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

/// Runs the 3 steps above.
///
/// # Errors
/// - `NotANeighbor` if the destination isn't next to the current planet
/// - `PlanetNotFound` / `ExplorerNotFound`
/// - `ChannelError` if a send fails or a planet doesn't answer in time
///
/// # Panics
/// If a mutex is poisoned.
pub fn execute(
    explorer_id: ID,
    current_planet_id: ID,
    dst_planet_id: ID,
    topology: &Arc<Mutex<Topology>>,
    planets: &Arc<Mutex<PlanetRegistry>>,
    explorers: &Arc<Mutex<ExplorerRegistry>>,
    planet_ack_rx: &Receiver<PlanetToOrchestrator>,
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

    // 1. current planet: the explorer is leaving
    {
        let planets = planets.lock().unwrap();

        let current = planets
            .get(current_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(current_planet_id))?;

        current
            .send(OrchestratorToPlanet::OutgoingExplorerRequest { explorer_id })
            .map_err(OrchestratorError::ChannelError)?;
    }

    wait_for_outgoing_ack(planet_ack_rx, current_planet_id, explorer_id)?;

    // 2. destination planet: the explorer is arriving
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

    wait_for_incoming_ack(planet_ack_rx, dst_planet_id, explorer_id)?;

    // 3. explorer: switch to the new planet
    let dst_explorer_tx = {
        let planets = planets.lock().unwrap();

        let dst = planets
            .get(dst_planet_id)
            .ok_or(OrchestratorError::PlanetNotFound(dst_planet_id))?;

        dst.explorer_sender()
    };

    {
        let explorers = explorers.lock().unwrap();

        let explorer = explorers
            .get(explorer_id)
            .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;

        explorer
            .send(OrchestratorToExplorer::MoveToPlanet {
                sender_to_new_planet: Some(dst_explorer_tx),
                planet_id: dst_planet_id,
            })
            .map_err(OrchestratorError::ChannelError)?;
    }

    log::info!(
        "MoveToPlanet sent to explorer {explorer_id}: {current_planet_id} → {dst_planet_id}"
    );

    Ok(())
}

fn wait_for_outgoing_ack(
    rx: &Receiver<PlanetToOrchestrator>,
    planet_id: ID,
    explorer_id: ID,
) -> Result<(), OrchestratorError> {
    recv_ack(
        rx,
        MOVE_TIMEOUT,
        &format!("OutgoingExplorerResponse from planet {planet_id} for explorer {explorer_id}"),
        |msg| match msg {
            PlanetToOrchestrator::OutgoingExplorerResponse { res, .. } => match res {
                Ok(()) => Ok(()),
                Err(err) => Err(PlanetToOrchestrator::OutgoingExplorerResponse {
                    res: Err(err),
                    planet_id,
                    explorer_id,
                }),
            },

            other => Err(other),
        },
    )
    .map_err(|err| match err {
        OrchestratorError::ChannelError(message) => OrchestratorError::ChannelError(message),
        other => other,
    })
}

fn wait_for_incoming_ack(
    rx: &Receiver<PlanetToOrchestrator>,
    planet_id: ID,
    explorer_id: ID,
) -> Result<(), OrchestratorError> {
    recv_ack(
        rx,
        MOVE_TIMEOUT,
        &format!("IncomingExplorerResponse from planet {planet_id} for explorer {explorer_id}"),
        |msg| match msg {
            PlanetToOrchestrator::IncomingExplorerResponse { res, .. } => match res {
                Ok(()) => Ok(()),
                Err(err) => Err(PlanetToOrchestrator::IncomingExplorerResponse {
                    res: Err(err),
                    planet_id,
                    explorer_id,
                }),
            },

            other => Err(other),
        },
    )
    .map_err(|err| match err {
        OrchestratorError::ChannelError(message) => OrchestratorError::ChannelError(message),
        other => other,
    })
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

        let result = execute(
            42, // explorer_id
            1,  // current_planet_id
            3,  // dst_planet_id, not a neighbor of 1
            &topo, &planets, &explorers, &planet_rx,
        );

        assert!(
            matches!(
                result,
                Err(OrchestratorError::NotANeighbor { from: 1, to: 3 })
            ),
            "expected NotANeighbor error, got {result:?}"
        );
    }
}
