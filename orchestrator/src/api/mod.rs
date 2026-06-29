//! # Orchestrator public API
//!
//! The `OrchestratorApi` provides the **synchronous** interface described in the
//! project spec. All methods block until the intended effect has taken place
//! (i.e. the relevant acknowledgment has been received).
//!
//! This is also the type that `main.rs` uses to wire up the interactive loop and
//! the visualizer.
//!
//! ## Owner: Vivi

pub mod commands;

use crate::error::OrchestratorError;
use crate::explorer::ExplorerRegistry;
use crate::galaxy::Topology;
use crate::logic::LogicController;
use crate::planet::PlanetRegistry;
use crate::probability::ProbabilityRegistry;
use common_game::components::forge::Forge;
use common_game::components::planet::DummyPlanetState;
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator,
    OrchestratorToExplorer,
};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const API_TIMEOUT: Duration = Duration::from_secs(10);

/// The main orchestrator API struct.
///
/// Owns all shared game state and provides the synchronous methods used by
/// `main.rs` (interactive loop) and the visualizer.
///
/// ## Construction
/// Constructed once in `main.rs` via [`OrchestratorApi::new`] after the galaxy
/// file has been parsed and all planet/explorer threads have been spawned.
pub struct OrchestratorApi {
    pub(crate) topology: Arc<Mutex<Topology>>,
    pub(crate) planets: Arc<Mutex<PlanetRegistry>>,
    pub(crate) explorers: Arc<Mutex<ExplorerRegistry>>,
    pub(crate) prob_registry: Arc<Mutex<ProbabilityRegistry>>,
    /// Shared receiver for all PlanetToOrchestrator messages.
    pub(crate) planet_rx: Arc<Mutex<Receiver<PlanetToOrchestrator>>>,
    /// Shared receiver for all ExplorerToOrchestrator messages.
    pub(crate) explorer_rx: Arc<Mutex<Receiver<crate::explorer::handle::ExplorerToOrchestratorMsg>>>,
    /// The forge used to generate sunrays and asteroids.
    pub(crate) forge: Forge,
    /// Controls the autonomous game logic thread.
    pub(crate) logic: LogicController,
}

impl OrchestratorApi {
    /// Starts the autonomous game logic loop.
    ///
    /// Once started:
    /// - Explorer AIs are started (via [`OrchestratorToExplorer::StartExplorerAI`]).
    /// - Sunrays and asteroids are sent automatically each tick.
    ///
    /// # Errors
    /// Returns [`OrchestratorError::InvalidState`] if already running.
    pub fn start_logic(&mut self) -> Result<(), OrchestratorError> {
        // Start all explorer AIs
        {
            let explorers = self.explorers.lock().unwrap();
            for h in explorers.iter() {
                h.send(OrchestratorToExplorer::StartExplorerAI)
                    .map_err(OrchestratorError::ChannelError)?;
            }
        }

        // Start all planet AIs
        {
            let planets = self.planets.lock().unwrap();
            for h in planets.iter() {
                h.send(OrchestratorToPlanet::StartPlanetAI)
                    .map_err(OrchestratorError::ChannelError)?;
            }
        }

        // Wait for all StartPlanetAIResult acks
        let mut pending_planets: std::collections::HashSet<ID> = {
            let planets = self.planets.lock().unwrap();
            planets.iter().map(|h| h.id()).collect()
        };

        while !pending_planets.is_empty() {
            let rx = self.planet_rx.lock().unwrap();

            match rx.recv_timeout(API_TIMEOUT) {
                Ok(PlanetToOrchestrator::StartPlanetAIResult { planet_id }) => {
                    pending_planets.remove(&planet_id);
                }

                Ok(other) => {
                    return Err(OrchestratorError::ChannelError(format!(
                        "Expected StartPlanetAIResult, got {other:?}"
                    )));
                }

                Err(_) => {
                    return Err(OrchestratorError::ChannelError(
                        "Timeout waiting for StartPlanetAIResult".to_string(),
                    ));
                }
            }
        }

        // Wait for all StartExplorerAIResult acks
        let mut pending_explorers: std::collections::HashSet<ID> = {
            let explorers = self.explorers.lock().unwrap();
            explorers.iter().map(|h| h.id()).collect()
        };

        while !pending_explorers.is_empty() {
            let rx = self.explorer_rx.lock().unwrap();

            match rx.recv_timeout(API_TIMEOUT) {
                Ok(ExplorerToOrchestrator::StartExplorerAIResult { explorer_id }) => {
                    pending_explorers.remove(&explorer_id);
                }

                Ok(other) => {
                    return Err(OrchestratorError::ChannelError(format!(
                        "Expected StartExplorerAIResult, got {other:?}"
                    )));
                }

                Err(_) => {
                    return Err(OrchestratorError::ChannelError(
                        "Timeout waiting for StartExplorerAIResult".to_string(),
                    ));
                }
            }
        }

        self.logic.start(
            Arc::clone(&self.topology),
            Arc::clone(&self.planets),
            Arc::clone(&self.explorers),
            Arc::clone(&self.prob_registry),
            Arc::clone(&self.planet_rx),
            Arc::clone(&self.explorer_rx),
        )
    }

    /// Stops the autonomous game logic loop and pauses all explorer AIs.
    ///
    /// # Errors
    /// Returns [`OrchestratorError::InvalidState`] if not running.
    pub fn stop_logic(&mut self) -> Result<(), OrchestratorError> {
        self.logic.stop()?;

        let explorers = self.explorers.lock().unwrap();
        for h in explorers.iter() {
            let _ = h.send(OrchestratorToExplorer::StopExplorerAI);
        }

        Ok(())
    }

    /// Manually sends a sunray to `planet_id` and waits for [`SunrayAck`].
    ///
    /// # Errors
    /// [`OrchestratorError::PlanetNotFound`] or [`OrchestratorError::ChannelError`].
    pub fn send_sunray(&self, planet_id: ID) -> Result<(), OrchestratorError> {
        let sunray = self.forge.generate_sunray();
        {
            let planets = self.planets.lock().unwrap();
            let planet = planets
                .get(planet_id)
                .ok_or(OrchestratorError::PlanetNotFound(planet_id))?;
            planet
                .send(OrchestratorToPlanet::Sunray(sunray))
                .map_err(OrchestratorError::ChannelError)?;
        }

        let rx = self.planet_rx.lock().unwrap();
        match rx.recv_timeout(API_TIMEOUT) {
            Ok(PlanetToOrchestrator::SunrayAck { planet_id: ack_id }) => {
                log::info!("Manual sunray: SunrayAck from planet {ack_id}");
                Ok(())
            }
            Ok(other) => Err(OrchestratorError::ChannelError(format!(
                "Expected SunrayAck, got {other:?}"
            ))),
            Err(_) => Err(OrchestratorError::ChannelError(format!(
                "Timeout waiting for SunrayAck from planet {planet_id}"
            ))),
        }
    }

    /// Manually sends an asteroid to `planet_id` and handles the result.
    ///
    /// If the planet cannot deflect it, the planet is destroyed.
    ///
    /// # Errors
    /// [`OrchestratorError::PlanetNotFound`] or [`OrchestratorError::ChannelError`].
    pub fn send_asteroid(&mut self, planet_id: ID) -> Result<bool, OrchestratorError> {
        let asteroid = self.forge.generate_asteroid();
        {
            let planets = self.planets.lock().unwrap();
            let planet = planets
                .get(planet_id)
                .ok_or(OrchestratorError::PlanetNotFound(planet_id))?;
            planet
                .send(OrchestratorToPlanet::Asteroid(asteroid))
                .map_err(OrchestratorError::ChannelError)?;
        }

        let ack = {
            let rx = self.planet_rx.lock().unwrap();
            rx.recv_timeout(API_TIMEOUT)
                .map_err(|_| OrchestratorError::ChannelError(format!("AsteroidAck timeout for planet {planet_id}")))?
        };

        match ack {
            PlanetToOrchestrator::AsteroidAck { rocket, .. } => {
                let survived = rocket.is_some();
                if !survived {
                    log::warn!("Planet {planet_id} destroyed by manual asteroid");
                    let mut planets = self.planets.lock().unwrap();
                    let mut topo = self.topology.lock().unwrap();
                    let mut prob = self.prob_registry.lock().unwrap();
                    crate::logic::tick::destroy_planet(planet_id, &mut planets, &mut topo, &mut prob, &mut rand::rng())?;
                }
                Ok(survived)
            }
            other => Err(OrchestratorError::ChannelError(format!(
                "Expected AsteroidAck, got {other:?}"
            ))),
        }
    }

    /// Manually moves `explorer_id` to `planet_id`.
    ///
    /// Validates that `planet_id` is a neighbor of the explorer's current planet.
    ///
    /// # Errors
    /// See [`crate::routing::move_explorer`].
    pub fn move_explorer(&self, explorer_id: ID, dst_planet_id: ID) -> Result<(), OrchestratorError> {
        // find where the explorer currently is
        let current_planet_id = {
            let explorers = self.explorers.lock().unwrap();
            explorers
                .get(explorer_id)
                .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?
                .current_planet()
        };

        // lock both receivers before calling execute, which needs raw references
        let planet_rx = self.planet_rx.lock().unwrap();
        let explorer_rx = self.explorer_rx.lock().unwrap();

        crate::routing::move_explorer::execute(
            explorer_id,
            current_planet_id,
            dst_planet_id,
            &self.topology,
            &self.planets,
            &self.explorers,
            &planet_rx,
            &explorer_rx,
        )
    }

    /// Asks `explorer_id` to generate a basic resource on its current planet.
    ///
    /// Returns `Ok(())` when the explorer confirms the resource was added to its bag.
    ///
    /// # Errors
    /// See channel errors.
    pub fn generate_resource(
        &self,
        explorer_id: ID,
        resource: common_game::components::resource::BasicResourceType,
    ) -> Result<(), OrchestratorError> {
        {
            let explorers = self.explorers.lock().unwrap();
            let h = explorers
                .get(explorer_id)
                .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;
            h.send(OrchestratorToExplorer::GenerateResourceRequest {
                to_generate: resource,
            })
            .map_err(OrchestratorError::ChannelError)?;
        }

        let rx = self.explorer_rx.lock().unwrap();

        match rx.recv_timeout(API_TIMEOUT) {
            Ok(ExplorerToOrchestrator::GenerateResourceResponse {
                   explorer_id: ack_id,
                   generated,
               }) if ack_id == explorer_id => {
                generated.map_err(OrchestratorError::ChannelError)
            }

            Ok(other) => Err(OrchestratorError::ChannelError(format!(
                "Expected GenerateResourceResponse from explorer {explorer_id}, got {other:?}"
            ))),

            Err(_) => Err(OrchestratorError::ChannelError(format!(
                "Timeout waiting for GenerateResourceResponse from explorer {explorer_id}"
            ))),
        }
    }

    /// Asks `explorer_id` to combine two resources on its current planet.
    ///
    /// # Errors
    /// See channel errors.
    pub fn combine_resource(
        &self,
        explorer_id: ID,
        resource: common_game::components::resource::ComplexResourceType,
    ) -> Result<(), OrchestratorError> {
        {
            let explorers = self.explorers.lock().unwrap();
            let h = explorers
                .get(explorer_id)
                .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;
            h.send(OrchestratorToExplorer::CombineResourceRequest {
                to_generate: resource,
            })
            .map_err(OrchestratorError::ChannelError)?;
        }

        let rx = self.explorer_rx.lock().unwrap();

        match rx.recv_timeout(API_TIMEOUT) {
            Ok(ExplorerToOrchestrator::CombineResourceResponse {
                   explorer_id: ack_id,
                   generated,
               }) if ack_id == explorer_id => {
                generated.map_err(OrchestratorError::ChannelError)
            }

            Ok(other) => Err(OrchestratorError::ChannelError(format!(
                "Expected CombineResourceResponse from explorer {explorer_id}, got {other:?}"
            ))),

            Err(_) => Err(OrchestratorError::ChannelError(format!(
                "Timeout waiting for CombineResourceResponse from explorer {explorer_id}"
            ))),
        }
    }

    /// Returns the internal state of `planet_id` (energy cells, rocket status).
    ///
    /// Used by the visualizer.
    ///
    /// # Errors
    /// [`OrchestratorError::PlanetNotFound`] or channel error.
    pub fn planet_state(&self, planet_id: ID) -> Result<DummyPlanetState, OrchestratorError> {
        {
            let planets = self.planets.lock().unwrap();
            let planet = planets
                .get(planet_id)
                .ok_or(OrchestratorError::PlanetNotFound(planet_id))?;
            planet
                .send(OrchestratorToPlanet::InternalStateRequest)
                .map_err(OrchestratorError::ChannelError)?;
        }

        let rx = self.planet_rx.lock().unwrap();
        match rx.recv_timeout(API_TIMEOUT) {
            Ok(PlanetToOrchestrator::InternalStateResponse { planet_state, .. }) => {
                Ok(planet_state)
            }
            Ok(other) => Err(OrchestratorError::ChannelError(format!(
                "Expected InternalStateResponse, got {other:?}"
            ))),
            Err(_) => Err(OrchestratorError::ChannelError(format!(
                "Timeout waiting for InternalStateResponse from planet {planet_id}"
            ))),
        }
    }

    /// Returns the IDs of neighbors of `planet_id`.
    pub fn neighbors(&self, planet_id: ID) -> Result<Vec<ID>, OrchestratorError> {
        let topo = self.topology.lock().unwrap();
        if !topo.contains(planet_id) {
            return Err(OrchestratorError::PlanetNotFound(planet_id));
        }
        Ok(topo.neighbors(planet_id))
    }

    /// Returns the IDs of all currently alive planets.
    pub fn alive_planets(&self) -> Vec<ID> {
        self.topology.lock().unwrap().planet_ids().collect()
    }

    /// Gracefully shuts down all threads: stops logic, kills all planets and explorers.
    pub fn shutdown(&mut self) {
        if self.logic.is_running() {
            let _ = self.logic.stop();
        }

        // Kill all planets
        let planet_ids: Vec<_> = {
            let planets = self.planets.lock().unwrap();
            planets.iter().map(|h| h.id()).collect()
        };
        for id in planet_ids {
            let mut planets = self.planets.lock().unwrap();
            if let Some(handle) = planets.get(id) {
                let _ = handle.send(OrchestratorToPlanet::KillPlanet);
            }
            if let Some(mut handle) = planets.remove(id) {
                handle.join();
            }
        }

        // Kill all explorers
        let explorer_ids: Vec<_> = {
            let explorers = self.explorers.lock().unwrap();
            explorers.iter().map(|h| h.id()).collect()
        };
        for id in explorer_ids {
            let mut explorers = self.explorers.lock().unwrap();
            if let Some(handle) = explorers.get(id) {
                let _ = handle.send(OrchestratorToExplorer::KillExplorer);
            }
            if let Some(mut handle) = explorers.remove(id) {
                handle.join();
            }
        }

        log::info!("Orchestrator shutdown complete");
    }
}
