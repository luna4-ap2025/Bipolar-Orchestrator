//! # Logic tick
//!
//! One tick of the game loop: decides sunray vs asteroid for a single planet
//! and handles the planet's acknowledgment (including destruction).
//!
//! ## Owner: Vivi

use crate::error::OrchestratorError;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use crate::probability::ProbabilityRegistry;
use common_game::components::forge::Forge;
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

        log::debug!("Sent sunray to planet {planet_id} (prob={sunray_prob:.2}, roll={roll:.2})");

        // wait for ack
        let ack = planet_ack_rx
            .recv_timeout(ACK_TIMEOUT)
            .map_err(|_| OrchestratorError::ChannelError(format!("Planet {planet_id} sunray ack timed out")))?;

        if let PlanetToOrchestrator::SunrayAck { planet_id: ack_id } = ack {
            log::debug!("SunrayAck from planet {ack_id}");
        }
    } else {
        let asteroid = forge.generate_asteroid();
        planet
            .send(OrchestratorToPlanet::Asteroid(asteroid))
            .map_err(OrchestratorError::ChannelError)?;

        log::info!(
            "Sent asteroid to planet {planet_id} (prob={:.2}, roll={roll:.2})",
            1.0 - sunray_prob
        );

        // wait for ack
        let ack = planet_ack_rx
            .recv_timeout(ACK_TIMEOUT)
            .map_err(|_| OrchestratorError::ChannelError(format!("Planet {planet_id} asteroid ack timed out")))?;

        match ack {
            PlanetToOrchestrator::AsteroidAck { planet_id: ack_id, rocket } => {
                if rocket.is_some() {
                    log::info!("Planet {ack_id} deflected the asteroid with a rocket");
                } else {
                    log::warn!("Planet {ack_id} has no rocket - it is destroyed");
                    destroy_planet(ack_id, planets, topology, prob_registry, rng)?;
                }
            }
            other => {
                log::warn!("Unexpected message while waiting for AsteroidAck: {other:?}");
            }
        }
    }

    Ok(())
}

/// Sends [`KillPlanet`] and removes the planet from all registries.
pub fn destroy_planet(
    planet_id: ID,
    planets: &mut PlanetRegistry,
    topology: &mut Topology,
    prob_registry: &mut ProbabilityRegistry,
    rng: &mut impl rand::Rng,
) -> Result<(), OrchestratorError> {
    // send kill
    if let Some(handle) = planets.get(planet_id) {
        let _ = handle.send(OrchestratorToPlanet::KillPlanet);
    }

    // remove from registries
    if let Some(mut handle) = planets.remove(planet_id) {
        handle.join();
    }

    topology.remove_planet(planet_id);
    prob_registry.on_planet_death(planet_id, rng);

    log::info!("Planet {planet_id} removed from galaxy. Bipolar mode: {:?}", prob_registry.bipolar_mode());
    Ok(())
}
