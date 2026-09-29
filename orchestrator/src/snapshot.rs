//! Builds the `GalaxySnapshot` the GUI reads (about 4 times a second).

use crate::api::OrchestratorApi;
use crate::probability::OrchestratorEvent;
use bipolar_shared::{ExplorerSnapshot, GalaxyEvent, GalaxySnapshot, Personality, ResourceKind};
use common_game::components::resource::{BasicResourceType, ComplexResourceType, ResourceType};
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;

// AIPartner is skipped: it can't be made with our planets, so there's no icon
// for it.
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

/// Reads the current state without waiting on any channel, so it can run
/// while the logic loop is going.
///
/// # Panics
/// If a mutex is poisoned.
#[must_use]
pub fn build(api: &OrchestratorApi) -> GalaxySnapshot {
    let (alive_planets, neighbors) = {
        let topo = api.topology.lock().unwrap();
        let ids: Vec<u32> = topo.planet_ids().collect();
        let neighbors = ids.iter().map(|&id| (id, topo.neighbors(id))).collect();
        (ids, neighbors)
    };

    // Uses the last bag the logic loop received (see ExplorerHandle::bag).
    // Waiting for the answer here ate the explorers' travel requests and they
    // got stuck. We still ask for a new bag each time, but don't wait: the
    // logic loop stores the answer when it arrives.
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
                ExplorerSnapshot {
                    id: h.id(),
                    planet: h.current_planet(),
                    bag,
                }
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
        let evts = prob
            .drain_events()
            .into_iter()
            .map(|e| match e {
                OrchestratorEvent::SunraySent { planet_id } => {
                    GalaxyEvent::SunraySent { planet_id }
                }
                OrchestratorEvent::SunrayReceived { planet_id } => {
                    GalaxyEvent::SunrayReceived { planet_id }
                }
                OrchestratorEvent::AsteroidSent { planet_id } => {
                    GalaxyEvent::AsteroidSent { planet_id }
                }
                OrchestratorEvent::AsteroidDeflected { planet_id } => {
                    GalaxyEvent::AsteroidDeflected { planet_id }
                }
                OrchestratorEvent::PlanetDestroyed { planet_id } => {
                    GalaxyEvent::PlanetDestroyed { planet_id }
                }
                OrchestratorEvent::ExplorerKilled { explorer_id } => {
                    GalaxyEvent::ExplorerKilled { explorer_id }
                }
                OrchestratorEvent::ExplorerMoved {
                    explorer_id,
                    from,
                    to,
                } => GalaxyEvent::ExplorerMoved {
                    explorer_id,
                    from,
                    to,
                },
            })
            .collect();
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
