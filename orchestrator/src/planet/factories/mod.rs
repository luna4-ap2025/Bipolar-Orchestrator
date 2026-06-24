//! # Planet factories
//!
//! Each external planet repo exposes a function (or type) for constructing its
//! planet. This module provides a common [`PlanetFactory`] trait so the spawner
//! can call any planet without knowing its concrete type.
//!
//! ## How to add a new planet
//! 1. Add the git dependency to `Cargo.toml`.
//! 2. Create `factories/<name>.rs` implementing [`PlanetFactory`].
//! 3. Add a `pub mod <name>` here.
//! 4. Register the factory in [`all_factories`].
//!
//! ## Owner: Vale (one factory file per external planet)

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

/// Factory trait implemented once per external planet crate.
///
/// The orchestrator calls [`PlanetFactory::create`] to build the planet and
/// then immediately spawns it in a thread.
pub trait PlanetFactory: Send + Sync {
    /// Returns the human-readable name of this planet (group name).
    fn name(&self) -> &'static str;

    /// Constructs the planet, wiring it to the provided channels.
    ///
    /// # Errors
    /// Returns a descriptive error string if construction fails (e.g. invalid
    /// parameters for the planet type constraints).
    fn create(
        &self,
        id: ID,
        rx_from_orchestrator: Receiver<common_game::protocols::orchestrator_planet::OrchestratorToPlanet>,
        tx_to_orchestrator: Sender<PlanetToOrchestrator>,
        rx_from_explorers: Receiver<ExplorerToPlanet>,
    ) -> Result<Planet, String>;
}

/// Returns one factory instance per external planet.
///
/// TODO(Vale): uncomment and add each factory as you integrate them.
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
