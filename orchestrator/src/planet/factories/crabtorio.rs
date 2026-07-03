//! # Crabtorio planet factory (crate name: `planet`)
//! ## Owner: Vale

use super::PlanetFactory;
use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub struct CrabtorioFactory;

impl PlanetFactory for CrabtorioFactory {
    fn name(&self) -> &'static str {
        "crabtorio"
    }

    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<OrchestratorToPlanet>,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<Planet, String> {
        Ok(planet::create_planet(
            id,
            rx_from_orchestrator,
            tx_to_orchestrator,
            rx_from_explorers,
        ))
    }
}
