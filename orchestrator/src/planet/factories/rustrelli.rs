//! Rustrelli (type D), generates all 4 basic resources.

use super::PlanetFactory;
use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub struct RustrelliFatory;

impl PlanetFactory for RustrelliFatory {
    fn name(&self) -> &'static str {
        "rustrelli"
    }

    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<OrchestratorToPlanet>,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<Planet, String> {
        // no limit on explorer requests
        Ok(rustrelli::create_planet(
            id,
            rx_from_orchestrator,
            tx_to_orchestrator,
            rx_from_explorers,
            rustrelli::ExplorerRequestLimit::None,
        ))
    }
}
