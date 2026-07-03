//! # Houston We Have A Borrow planet factory — Type A, configurable resource + rocket strategy
//!
//! The crate exposes `houston_we_have_a_borrow()` with two extra params:
//! - `RocketStrategy` — how aggressively to build/reload rockets
//! - `basic_resource` — which single basic resource this planet generates
//!
//! We default to `RocketStrategy::Default` and `None` (crate default: Hydrogen).
//!
//! Note: the dependency crate is named `Planet` (capital P), which conflicts with
//! `common_game::components::planet::Planet`. We avoid the conflict by using full paths.
//!
//! ## Owner: Vale

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
