//! # Logic tick
//!
//! One tick of the game loop: decides sunray vs asteroid for a single planet
//! and handles the planet's acknowledgment (including destruction).
//!
//! ## Owner: Vivi

use crate::ack::recv_ack;
use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::explorer::handle::ExplorerToOrchestratorMsg;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use crate::probability::{OrchestratorEvent, ProbabilityRegistry};
use common_game::components::forge::Forge;
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::utils::ID;
use crossbeam_channel::Receiver;
use std::time::Duration;
use rand::RngExt;

/// Timeout when waiting for a planet's acknowledgment.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends either a sunray or asteroid to `planet_id` based on the probability
/// curve, then waits for the ack.
///
/// If the asteroid is not deflected (`rocket: None`), the planet is killed:
/// - [`OrchestratorToPlanet::KillPlanet`] is sent.
/// - The planet is removed from `planets` and `topology`.
/// - [`ProbabilityRegistry::on_planet_death`] is called.
///
/// # Errors
/// Returns an error if a channel send/receive fails or times out.
pub fn dispatch_to_planet(
    planet_id: ID,
    forge: &Forge,
    prob_registry: &mut ProbabilityRegistry,
    planets: &mut PlanetRegistry,
    topology: &mut Topology,
    planet_ack_rx: &Receiver<PlanetToOrchestrator>,
    explorers: &mut ExplorerRegistry,
    explorer_ack_rx: &Receiver<ExplorerToOrchestratorMsg>,
    rng: &mut impl rand::Rng,
) -> Result<(), OrchestratorError> {
    let sunray_prob = prob_registry.sunray_probability(planet_id);
    let roll: f64 = rng.random();

    let planet = planets
        .get(planet_id)
        .ok_or(OrchestratorError::PlanetNotFound(planet_id))?;

    if roll < sunray_prob {
        let sunray = forge.generate_sunray();
        planet
            .send(OrchestratorToPlanet::Sunray(sunray))
            .map_err(OrchestratorError::ChannelError)?;

        prob_registry.record_event(OrchestratorEvent::SunraySent { planet_id });
        log::debug!("Sent sunray to planet {planet_id} (prob={sunray_prob:.2}, roll={roll:.2})");

        // Wait for this specific planet's ack, discarding any stray messages
        // (e.g. a straggling KillPlanetResult from a planet that just died).
        let ack_id = recv_ack(
            planet_ack_rx,
            ACK_TIMEOUT,
            &format!("SunrayAck from planet {planet_id}"),
            |msg| match msg {
                PlanetToOrchestrator::SunrayAck { planet_id: id } if id == planet_id => Ok(id),
                other => Err(other),
            },
        )?;
        prob_registry.record_event(OrchestratorEvent::SunrayReceived { planet_id: ack_id });
        log::debug!("SunrayAck from planet {ack_id}");
    } else {
        let asteroid = forge.generate_asteroid();
        planet
            .send(OrchestratorToPlanet::Asteroid(asteroid))
            .map_err(OrchestratorError::ChannelError)?;

        prob_registry.record_event(OrchestratorEvent::AsteroidSent { planet_id });
        log::info!(
            "Sent asteroid to planet {planet_id} (prob={:.2}, roll={roll:.2})",
            1.0 - sunray_prob
        );

        // Wait for this specific planet's ack, discarding any stray messages.
        let (ack_id, rocket) = recv_ack(
            planet_ack_rx,
            ACK_TIMEOUT,
            &format!("AsteroidAck from planet {planet_id}"),
            |msg| match msg {
                PlanetToOrchestrator::AsteroidAck { planet_id: id, rocket } if id == planet_id => {
                    Ok((id, rocket))
                }
                other => Err(other),
            },
        )?;

        if rocket.is_some() {
            prob_registry.record_event(OrchestratorEvent::AsteroidDeflected { planet_id: ack_id });
            log::info!("Planet {ack_id} deflected the asteroid with a rocket");
        } else {
            log::warn!("Planet {ack_id} has no rocket - it is destroyed");
            destroy_planet(ack_id, planets, topology, prob_registry, explorers, explorer_ack_rx, rng)?;
        }
    }

    Ok(())
}

/// Timeout for relocating an explorer stranded by a planet's destruction.
const RELOCATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends [`KillPlanet`], removes the planet from all registries, and
/// relocates any explorer that was standing on it (otherwise the explorer's
/// `current_planet` would keep pointing at a planet that no longer exists —
/// silently stale everywhere it's read, including the GUI).
pub fn destroy_planet(
    planet_id: ID,
    planets: &mut PlanetRegistry,
    topology: &mut Topology,
    prob_registry: &mut ProbabilityRegistry,
    explorers: &mut ExplorerRegistry,
    explorer_ack_rx: &Receiver<ExplorerToOrchestratorMsg>,
    rng: &mut impl rand::Rng,
) -> Result<(), OrchestratorError> {
    // Capture neighbors before the planet is removed from the topology.
    let neighbors = topology.neighbors(planet_id);

    // send kill
    if let Some(handle) = planets.get(planet_id) {
        let _ = handle.send(OrchestratorToPlanet::KillPlanet);
    }

    // remove from registries
    if let Some(mut handle) = planets.remove(planet_id) {
        handle.join();
    }

    topology.remove_planet(planet_id);
    prob_registry.record_event(OrchestratorEvent::PlanetDestroyed { planet_id });
    prob_registry.on_planet_death(planet_id, rng);

    // Relocate any explorer that was stationed on the destroyed planet.
    let stranded: Vec<ID> = explorers
        .iter()
        .filter(|h| h.current_planet() == planet_id)
        .map(|h| h.id())
        .collect();

    for explorer_id in stranded {
        let destination = neighbors
            .iter()
            .copied()
            .find(|id| planets.get(*id).is_some())
            .or_else(|| planets.iter().map(|h| h.id()).next());

        let Some(dst) = destination else {
            log::warn!("Explorer {explorer_id} stranded - no surviving planets left");
            continue;
        };

        if let Err(e) = relocate_stranded_explorer(explorer_id, dst, planets, explorers, explorer_ack_rx) {
            log::error!("Failed to relocate stranded explorer {explorer_id} to planet {dst}: {e}");
        }
    }

    log::info!("Planet {planet_id} removed from galaxy. Bipolar mode: {:?}", prob_registry.bipolar_mode());
    Ok(())
}

/// Moves `explorer_id` to `dst_planet_id` without asking their old planet to
/// release them first (it's already destroyed and gone). Mirrors steps 2-3 of
/// [`crate::routing::move_explorer::execute`].
fn relocate_stranded_explorer(
    explorer_id: ID,
    dst_planet_id: ID,
    planets: &PlanetRegistry,
    explorers: &mut ExplorerRegistry,
    explorer_ack_rx: &Receiver<ExplorerToOrchestratorMsg>,
) -> Result<(), OrchestratorError> {
    let planet_reply_tx = explorers
        .get(explorer_id)
        .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?
        .planet_reply_tx();

    let dst = planets
        .get(dst_planet_id)
        .ok_or(OrchestratorError::PlanetNotFound(dst_planet_id))?;

    dst.send(OrchestratorToPlanet::IncomingExplorerRequest {
        explorer_id,
        new_sender: planet_reply_tx,
    })
    .map_err(OrchestratorError::ChannelError)?;

    let dst_explorer_tx = dst.explorer_sender();
    let expl = explorers
        .get(explorer_id)
        .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;
    expl.send(OrchestratorToExplorer::MoveToPlanet {
        sender_to_new_planet: Some(dst_explorer_tx),
        planet_id: dst_planet_id,
    })
    .map_err(OrchestratorError::ChannelError)?;

    recv_ack(
        explorer_ack_rx,
        RELOCATE_TIMEOUT,
        &format!("MovedToPlanetResult for stranded explorer {explorer_id}"),
        |msg| match msg {
            ExplorerToOrchestrator::MovedToPlanetResult { explorer_id: eid, planet_id } if eid == explorer_id => {
                Ok(planet_id)
            }
            other => Err(other),
        },
    )?;

    if let Some(h) = explorers.get_mut(explorer_id) {
        h.set_current_planet(dst_planet_id);
    }
    log::info!("Explorer {explorer_id} relocated to planet {dst_planet_id} after their planet was destroyed");
    Ok(())
}
