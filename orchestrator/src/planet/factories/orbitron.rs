//! # Orbitron planet factory
//!
//! Wraps the `orbitron` crate's planet constructor in the [`PlanetFactory`] trait.
//!
//! ## Steps to activate
//! 1. Uncomment the `orbitron` dependency in `Cargo.toml`.
//! 2. Check the actual public API of the orbitron crate (function name,
//!    parameter order, which `PlanetType` it uses).
//! 3. Fill in the `create` body below.
//! 4. Uncomment `pub mod orbitron` and the factory entry in `all_factories()`.
//!
//! ## Owner: Vale

use super::PlanetFactory;
use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

/// Factory for the Orbitron planet (group: Orbitron).
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
        // TODO(Vale): call orbitron's public factory function.
        // Example (adjust to actual API):
        //
        //   orbitron::create_planet(
        //       id,
        //       rx_from_orchestrator,
        //       tx_to_orchestrator,
        //       rx_from_explorers,
        //   )
        //
        Err("Orbitron factory not implemented yet - fill in the create() body above".to_string())
    }
}

// Copy this file to skycartel.rs, rustrelli.rs, etc. and adjust accordingly.
