//! Orbitron.

use super::PlanetFactory;
use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub struct OrbitronFactory;

impl PlanetFactory for OrbitronFactory {
    fn name(&self) -> &'static str {
        "Orbitron"
    }

    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<OrchestratorToPlanet>,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<Planet, String> {
        Ok(orbitron::create_planet(
            rx_from_orchestrator,
            tx_to_orchestrator,
            rx_from_explorers,
            id,
        ))
    }
}
