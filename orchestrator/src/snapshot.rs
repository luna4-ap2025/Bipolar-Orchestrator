use crate::api::OrchestratorApi;
use crate::probability::OrchestratorEvent;
use bipolar_shared::{ExplorerSnapshot, GalaxyEvent, GalaxySnapshot, Personality, ResourceKind};
use common_game::components::resource::{BasicResourceType, ComplexResourceType, ResourceType};
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;

/// Maps the real explorer-bag resource type onto the shared, dependency-free
/// `ResourceKind` the GUI reads. `AIPartner` has no GUI icon asset (it isn't
/// reachable via this galaxy's recipes) and is dropped rather than faked with
/// a placeholder.
fn resource_kind(rt: ResourceType) -> Option<ResourceKind> {
    match rt {
        ResourceType::Basic(BasicResourceType::Oxygen) => Some(ResourceKind::Oxygen),
        ResourceType::Basic(BasicResourceType::Hydrogen) => Some(ResourceKind::Hydrogen),
        ResourceType::Basic(BasicResourceType::Carbon) => Some(ResourceKind::Carbon),
        ResourceType::Basic(BasicResourceType::Silicon) => Some(ResourceKind::Silicon),
        ResourceType::Complex(ComplexResourceType::Diamond) => Some(ResourceKind::Diamond),
        ResourceType::Complex(ComplexResourceType::Water) => Some(ResourceKind::Water),
        ResourceType::Complex(ComplexResourceType::Life) => Some(ResourceKind::Life),
        ResourceType::Complex(ComplexResourceType::Robot) => Some(ResourceKind::Robot),
        ResourceType::Complex(ComplexResourceType::Dolphin) => Some(ResourceKind::Dolphin),
        ResourceType::Complex(ComplexResourceType::AIPartner) => None,
    }
}

/// Reads the current game state from the API's shared registries without
/// sending any channel messages. Safe to call from a background thread
/// while the logic loop is running.
pub fn build(api: &OrchestratorApi) -> GalaxySnapshot {
    let (alive_planets, neighbors) = {
        let topo = api.topology.lock().unwrap();
        let ids: Vec<u32> = topo.planet_ids().collect();
        let neighbors = ids.iter().map(|&id| (id, topo.neighbors(id))).collect();
        (ids, neighbors)
    };

    // Bag content is read from each handle's cache (last `BagContentResponse`
    // the logic loop's own drain has seen — see `ExplorerHandle::bag`) rather
    // than fetched with a blocking round-trip on `explorer_rx`. That channel
    // is also where autonomous `NeighborsRequest`/`TravelToPlanetRequest`
    // messages arrive, and a second blocking reader competing for them would
    // (and, before this fix, did) silently swallow those as "stray" acks,
    // stalling explorer movement. A fresh `BagContentRequest` is still fired
    // every poll — just fire-and-forget, so it costs a channel *send*, not a
    // wait — and its reply gets cached by `event_handler::handle_one` the
    // next time the logic loop drains (asynchronously, no lock contention).
    let explorers = {
        let reg = api.explorers.lock().unwrap();
        reg.iter()
            .map(|h| {
                let _ = h.send(OrchestratorToExplorer::BagContentRequest);
                let bag = h
                    .bag()
                    .iter()
                    .filter_map(|&(rt, n)| resource_kind(rt).map(|k| (k, n)))
                    .collect();
                ExplorerSnapshot { id: h.id(), planet: h.current_planet(), bag }
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
            OrchestratorEvent::ExplorerKilled { explorer_id } => GalaxyEvent::ExplorerKilled { explorer_id },
            OrchestratorEvent::ExplorerMoved { explorer_id, from, to } => GalaxyEvent::ExplorerMoved { explorer_id, from, to },
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
        neighbors,
    }
}
