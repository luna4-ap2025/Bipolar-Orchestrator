use crate::api::OrchestratorApi;
use crate::probability::OrchestratorEvent;
use bipolar_shared::{ExplorerSnapshot, GalaxyEvent, GalaxySnapshot, Personality};

/// Reads the current game state from the API's shared registries without
/// sending any channel messages. Safe to call from a background thread
/// while the logic loop is running.
pub fn build(api: &OrchestratorApi) -> GalaxySnapshot {
    let alive_planets = api.topology.lock().unwrap().planet_ids().collect();

    let explorers = {
        let reg = api.explorers.lock().unwrap();
        reg.iter()
            .map(|h| ExplorerSnapshot {
                id: h.id(),
                planet: h.current_planet(),
            })
            .collect()
    };

    let (personality, hostility, phase_elapsed, events) = {
        let mut prob = api.prob_registry.lock().unwrap();
        let p = if prob.bipolar_mode().is_eclipse() {
            Personality::Eclipse
        } else {
            Personality::Solace
        };
        let h = prob.hostility();
        let pe = prob.phase_elapsed();
        let evts = prob.drain_events().into_iter().map(|e| match e {
            OrchestratorEvent::SunraySent        { planet_id } => GalaxyEvent::SunraySent        { planet_id },
            OrchestratorEvent::SunrayReceived    { planet_id } => GalaxyEvent::SunrayReceived    { planet_id },
            OrchestratorEvent::AsteroidSent      { planet_id } => GalaxyEvent::AsteroidSent      { planet_id },
            OrchestratorEvent::AsteroidDeflected { planet_id } => GalaxyEvent::AsteroidDeflected { planet_id },
            OrchestratorEvent::PlanetDestroyed   { planet_id } => GalaxyEvent::PlanetDestroyed   { planet_id },
        }).collect();
        (p, h, pe, evts)
    };

    GalaxySnapshot {
        personality,
        hostility,
        phase_elapsed,
        alive_planets,
        explorers,
        events,
    }
}
