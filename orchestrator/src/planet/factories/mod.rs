//! Every group builds their planet in a different way, so each one gets a
//! small wrapper implementing [`PlanetFactory`] and the spawner doesn't care
//! which planet it is.
//!
//! Adding a planet: add its git dependency in Cargo.toml, make a file here
//! that implements `PlanetFactory`, add the `pub mod`, and add it to
//! [`all_factories`].

use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub mod crabtorio;
pub mod enterprise;
pub mod houstonwehaveaborrow;
pub mod orbitron;
pub mod rustrelli;
pub mod skycartel;
pub mod thecompilerstrikesback;

pub trait PlanetFactory: Send + Sync {
    /// The group's name.
    fn name(&self) -> &'static str;

    /// Builds the planet with the given channels.
    ///
    /// # Errors
    /// If the group's constructor fails.
    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<
            common_game::protocols::orchestrator_planet::OrchestratorToPlanet,
        >,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<Planet, String>;
}

#[must_use]
pub fn all_factories() -> Vec<Box<dyn PlanetFactory>> {
    vec![
        Box::new(orbitron::OrbitronFactory),
        Box::new(skycartel::SkycartelFactory),
        Box::new(rustrelli::RustrelliFatory),
        Box::new(thecompilerstrikesback::TheCompilerStrikesBackFactory),
        Box::new(crabtorio::CrabtorioFactory),
        Box::new(houstonwehaveaborrow::HoustonFactory),
        Box::new(enterprise::EnterpriseFactory),
    ]
}
