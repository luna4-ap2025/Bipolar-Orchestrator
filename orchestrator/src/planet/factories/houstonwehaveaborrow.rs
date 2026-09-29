//! Houston We Have A Borrow (type A). Their constructor also takes a
//! `RocketStrategy` and which basic resource to generate. We use the default
//! strategy and `None` (Hydrogen).
//!
//! Their crate is called `Planet`, same name as `common_game`'s `Planet`, so
//! full paths are used here.

use super::PlanetFactory;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub struct HoustonFactory;

impl PlanetFactory for HoustonFactory {
    fn name(&self) -> &'static str {
        "houstonwehaveaborrow"
    }

    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<OrchestratorToPlanet>,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<common_game::components::planet::Planet, String> {
        Planet::houston_we_have_a_borrow(
            rx_from_orchestrator,
            tx_to_orchestrator,
            rx_from_explorers,
            id,
            Planet::RocketStrategy::Default,
            None,
        )
    }
}
