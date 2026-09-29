//! The Compiler Strikes Back: generates Silicon, combines Robot, AI partner
//! and Diamond.

use super::PlanetFactory;
use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub struct TheCompilerStrikesBackFactory;

impl PlanetFactory for TheCompilerStrikesBackFactory {
    fn name(&self) -> &'static str {
        "thecompilerstrikesback"
    }

    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<OrchestratorToPlanet>,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<Planet, String> {
        Ok(the_compiler_strikes_back::planet::create_planet(
            rx_from_orchestrator,
            tx_to_orchestrator,
            rx_from_explorers,
            id,
        ))
    }
}
