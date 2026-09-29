//! Sending a sunray or an asteroid to one planet, and destroying it if it
//! can't defend itself.

use crate::ack::recv_ack;
use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::galaxy::Topology;
use crate::planet::PlanetRegistry;
use crate::probability::{OrchestratorEvent, ProbabilityRegistry};
use common_game::components::forge::Forge;
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::utils::ID;
use crossbeam_channel::Receiver;
use rand::RngExt;
use std::time::Duration;

const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Rolls against the planet's curve, sends a sunray or an asteroid and waits
/// for the ack. If an asteroid isn't deflected the planet is destroyed.
///
/// Returns `true` if the planet was destroyed. The loop stops the tick there,
/// otherwise several planets could die at the same moment.
///
/// # Errors
/// If a send fails or the ack times out.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_to_planet(
    planet_id: ID,
    forge: &Forge,
    prob_registry: &mut ProbabilityRegistry,
    planets: &mut PlanetRegistry,
    topology: &mut Topology,
    planet_ack_rx: &Receiver<PlanetToOrchestrator>,
    explorers: &mut ExplorerRegistry,
    rng: &mut impl rand::Rng,
) -> Result<bool, OrchestratorError> {
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
        Ok(false)
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

        let (ack_id, rocket) = recv_ack(
            planet_ack_rx,
            ACK_TIMEOUT,
            &format!("AsteroidAck from planet {planet_id}"),
            |msg| match msg {
                PlanetToOrchestrator::AsteroidAck {
                    planet_id: id,
                    rocket,
                } if id == planet_id => Ok((id, rocket)),
                other => Err(other),
            },
        )?;

        if rocket.is_some() {
            prob_registry.record_event(OrchestratorEvent::AsteroidDeflected { planet_id: ack_id });
            log::info!("Planet {ack_id} deflected the asteroid with a rocket");
            Ok(false)
        } else {
            log::warn!("Planet {ack_id} has no rocket - it is destroyed");
            destroy_planet(ack_id, planets, topology, prob_registry, explorers, rng)?;
            Ok(true)
        }
    }
}

/// Kills the planet, removes it everywhere, and kills any explorer that was
/// on it (we decided explorers die with their planet instead of escaping to
/// a neighbor).
///
/// `join()` waits until the explorer thread really ends, so there's no need
/// to wait for `KillExplorerResult`.
///
/// # Errors
/// Never fails for now. It returns a Result so callers don't have to change
/// if it ever needs to.
pub fn destroy_planet(
    planet_id: ID,
    planets: &mut PlanetRegistry,
    topology: &mut Topology,
    prob_registry: &mut ProbabilityRegistry,
    explorers: &mut ExplorerRegistry,
    rng: &mut impl rand::Rng,
) -> Result<(), OrchestratorError> {
    if let Some(handle) = planets.get(planet_id) {
        let _ = handle.send(OrchestratorToPlanet::KillPlanet);
    }

    if let Some(mut handle) = planets.remove(planet_id) {
        handle.join();
    }

    topology.remove_planet(planet_id);
    prob_registry.record_event(OrchestratorEvent::PlanetDestroyed { planet_id });
    // read before on_planet_death lowers hostility, otherwise the log always
    // says Solace even when Eclipse was the one who killed it
    let mode_at_death = prob_registry.bipolar_mode();
    prob_registry.on_planet_death(planet_id, rng);

    let stranded: Vec<ID> = explorers
        .iter()
        .filter(|h| h.current_planet() == planet_id)
        .map(super::super::explorer::handle::ExplorerHandle::id)
        .collect();

    for explorer_id in stranded {
        if let Some(handle) = explorers.get(explorer_id) {
            let _ = handle.send(OrchestratorToExplorer::KillExplorer);
        }
        if let Some(mut handle) = explorers.remove(explorer_id) {
            handle.join();
        }
        prob_registry.record_event(OrchestratorEvent::ExplorerKilled { explorer_id });
        log::info!("Explorer {explorer_id} died along with planet {planet_id}");
    }

    log::info!(
        "Planet {planet_id} removed from galaxy. Killed under: {mode_at_death:?} (now dampened to {:?})",
        prob_registry.bipolar_mode()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::ExplorerHandle;
    use common_game::protocols::planet_explorer::PlanetToExplorer;
    use crossbeam_channel::unbounded;

    // the thread ends right away so join() in destroy_planet doesn't block
    fn fake_explorer(id: ID, planet_id: ID) -> ExplorerHandle {
        let (tx, _rx) = unbounded::<OrchestratorToExplorer>();
        let (planet_reply_tx, _rx2) = unbounded::<PlanetToExplorer>();
        let thread = std::thread::spawn(|| {});
        ExplorerHandle::new(id, tx, planet_reply_tx, planet_id, thread)
    }

    #[test]
    fn destroy_planet_kills_only_the_stranded_explorer() {
        let mut planets = PlanetRegistry::new();
        let mut topology = crate::galaxy::parser::parse_str("1 2\n2 1\n").unwrap();
        let mut prob = ProbabilityRegistry::new([1, 2].into_iter(), &mut rand::rng());
        let mut explorers = ExplorerRegistry::new();
        explorers.insert(fake_explorer(1, 1)); // stationed on the doomed planet
        explorers.insert(fake_explorer(2, 2)); // stationed elsewhere

        destroy_planet(
            1,
            &mut planets,
            &mut topology,
            &mut prob,
            &mut explorers,
            &mut rand::rng(),
        )
        .expect("destroy_planet should succeed");

        assert!(
            explorers.get(1).is_none(),
            "explorer on the destroyed planet should be killed"
        );
        assert!(
            explorers.get(2).is_some(),
            "explorer on a surviving planet should be untouched"
        );
    }

    #[test]
    fn destroy_planet_records_explorer_killed_event() {
        let mut planets = PlanetRegistry::new();
        let mut topology = crate::galaxy::parser::parse_str("1 2\n2 1\n").unwrap();
        let mut prob = ProbabilityRegistry::new([1, 2].into_iter(), &mut rand::rng());
        let mut explorers = ExplorerRegistry::new();
        explorers.insert(fake_explorer(7, 1));

        destroy_planet(
            1,
            &mut planets,
            &mut topology,
            &mut prob,
            &mut explorers,
            &mut rand::rng(),
        )
        .expect("destroy_planet should succeed");

        let events = prob.drain_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OrchestratorEvent::ExplorerKilled { explorer_id: 7 })),
            "expected an ExplorerKilled event for explorer 7, got {events:?}"
        );
    }

    #[test]
    fn destroy_planet_with_no_explorers_present_is_a_no_op() {
        let mut planets = PlanetRegistry::new();
        let mut topology = crate::galaxy::parser::parse_str("1 2\n2 1\n").unwrap();
        let mut prob = ProbabilityRegistry::new([1, 2].into_iter(), &mut rand::rng());
        let mut explorers = ExplorerRegistry::new();

        let result = destroy_planet(
            1,
            &mut planets,
            &mut topology,
            &mut prob,
            &mut explorers,
            &mut rand::rng(),
        );
        assert!(result.is_ok());
        assert!(explorers.is_empty());
    }
}
