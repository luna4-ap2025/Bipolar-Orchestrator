use crate::api::OrchestratorApi;
use crate::explorer::{ExplorerHandle, ExplorerRegistry};
use crate::galaxy;
use crate::planet::{self, PlanetRegistry};
use crate::probability::ProbabilityRegistry;

use common_game::components::forge::Forge;
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;
use common_game::protocols::planet_explorer::PlanetToExplorer;

use crossbeam_channel::unbounded;
use std::collections::HashMap;

/// Reads the galaxy and planet config, spawns all the planets and both
/// explorers (Viviana, id 1, on planet 4 and Jeb, id 2, on planet 1).
///
/// # Panics
/// If one of the files is wrong, a planet can't be created, or the `Forge`
/// was already created. It only runs once at startup, and there's no way to
/// play with a broken galaxy anyway.
#[must_use]
pub fn build_api(galaxy_src: &str, planets_src: &str) -> OrchestratorApi {
    let topology = galaxy::parser::parse_str(galaxy_src).expect("invalid galaxy source");
    let planet_config =
        galaxy::planet_config::parse_str(planets_src).expect("invalid planets source");

    let (planet_tx, planet_rx) = unbounded::<PlanetToOrchestrator>();

    let factories: HashMap<String, Box<dyn planet::factories::PlanetFactory>> =
        planet::factories::all_factories()
            .into_iter()
            .map(|f| (f.name().to_lowercase(), f))
            .collect();

    let mut planet_registry = PlanetRegistry::new();
    let planet_ids: Vec<_> = topology.planet_ids().collect();

    for &id in &planet_ids {
        let factory_name = planet_config.get(&id).expect("planet has no factory entry");
        let factory = factories
            .get(&factory_name.to_lowercase())
            .expect("unknown planet factory");
        let handle = planet::spawn_planet(id, factory.name(), factory.as_ref(), planet_tx.clone())
            .expect("failed to spawn planet");
        planet_registry.insert(handle);
    }

    let (explorer_tx, explorer_rx) =
        unbounded::<crate::explorer::handle::ExplorerToOrchestratorMsg>();
    let mut explorer_registry = ExplorerRegistry::new();

    // Viviana
    {
        let (tx_to_viv, rx_from_orch) = unbounded::<OrchestratorToExplorer>();
        let (planet_reply_tx, rx_from_planet) = unbounded::<PlanetToExplorer>();
        let tx_planet = planet_registry
            .get(4)
            .expect("planet 4 not spawned")
            .explorer_sender();
        let mut viv = explorer_astronaut::create_explorer(
            1,
            rx_from_orch,
            explorer_tx.clone(),
            rx_from_planet,
            tx_planet,
            4,
        )
        .expect("failed to create Viviana");
        let thread = std::thread::Builder::new()
            .name("explorer-viviana".into())
            .spawn(move || viv.run())
            .expect("failed to spawn Viviana thread");
        explorer_registry.insert(ExplorerHandle::new(
            1,
            tx_to_viv,
            planet_reply_tx,
            4,
            thread,
        ));
    }

    // Jeb
    {
        let (tx_to_jeb, rx_from_orch) = unbounded::<OrchestratorToExplorer>();
        let (planet_reply_tx, rx_from_planet) = unbounded::<PlanetToExplorer>();
        let tx_planet = planet_registry
            .get(1)
            .expect("planet 1 not spawned")
            .explorer_sender();
        let jeb = explorer_jebediah::create_explorer(
            2,
            rx_from_orch,
            explorer_tx.clone(),
            rx_from_planet,
            tx_planet,
            1,
        )
        .expect("failed to create Jeb");
        let thread = std::thread::Builder::new()
            .name("explorer-jeb".into())
            .spawn(move || {
                if let Err(e) = jeb.run() {
                    log::error!("Jeb explorer thread exited with error: {e}");
                }
            })
            .expect("failed to spawn Jeb thread");
        explorer_registry.insert(ExplorerHandle::new(
            2,
            tx_to_jeb,
            planet_reply_tx,
            1,
            thread,
        ));
    }

    let mut rng = rand::rng();
    let prob_registry = ProbabilityRegistry::new(planet_ids.iter().copied(), &mut rng);
    let forge = Forge::new().expect("failed to create Forge");

    OrchestratorApi::new(
        topology,
        planet_registry,
        explorer_registry,
        prob_registry,
        planet_rx,
        explorer_rx,
        forge,
    )
}
