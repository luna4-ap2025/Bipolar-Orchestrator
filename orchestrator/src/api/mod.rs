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

use crate::ack::recv_ack;
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
use common_game::utils::ID;
use crossbeam_channel::Receiver;
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
    pub(crate) forge: Arc<Mutex<Forge>>,
    /// Controls the autonomous game logic thread.
    pub(crate) logic: LogicController,
}

impl OrchestratorApi {
    /// Creates a new orchestrator API from already-spawned game components.
    pub fn new(
        topology: Topology,
        planets: PlanetRegistry,
        explorers: ExplorerRegistry,
        prob_registry: ProbabilityRegistry,
        planet_rx: Receiver<PlanetToOrchestrator>,
        explorer_rx: Receiver<crate::explorer::handle::ExplorerToOrchestratorMsg>,
        forge: Forge,
    ) -> Self {
        Self {
            topology: Arc::new(Mutex::new(topology)),
            planets: Arc::new(Mutex::new(planets)),
            explorers: Arc::new(Mutex::new(explorers)),
            prob_registry: Arc::new(Mutex::new(prob_registry)),
            planet_rx: Arc::new(Mutex::new(planet_rx)),
            explorer_rx: Arc::new(Mutex::new(explorer_rx)),
            forge: Arc::new(Mutex::new(forge)),
            logic: LogicController::new(),
        }
    }

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

        // Wait for all StartPlanetAIResult acks, discarding stray messages
        // left over on the shared receiver rather than misreading them.
        let mut pending_planets: std::collections::HashSet<ID> = {
            let planets = self.planets.lock().unwrap();
            planets.iter().map(|h| h.id()).collect()
        };

        while !pending_planets.is_empty() {
            let rx = self.planet_rx.lock().unwrap();
            let planet_id = recv_ack(
                &rx,
                API_TIMEOUT,
                "StartPlanetAIResult",
                |msg| match msg {
                    PlanetToOrchestrator::StartPlanetAIResult { planet_id }
                        if pending_planets.contains(&planet_id) =>
                    {
                        Ok(planet_id)
                    }
                    other => Err(other),
                },
            )?;
            pending_planets.remove(&planet_id);
        }

        // Wait for all StartExplorerAIResult acks, discarding stray messages
        // (e.g. a leftover StopExplorerAIResult from a just-preceding pause)
        // rather than misreading them as a fatal protocol error.
        let mut pending_explorers: std::collections::HashSet<ID> = {
            let explorers = self.explorers.lock().unwrap();
            explorers.iter().map(|h| h.id()).collect()
        };

        while !pending_explorers.is_empty() {
            let rx = self.explorer_rx.lock().unwrap();
            let explorer_id = recv_ack(
                &rx,
                API_TIMEOUT,
                "StartExplorerAIResult",
                |msg| match msg {
                    ExplorerToOrchestrator::StartExplorerAIResult { explorer_id }
                        if pending_explorers.contains(&explorer_id) =>
                    {
                        Ok(explorer_id)
                    }
                    other => Err(other),
                },
            )?;
            pending_explorers.remove(&explorer_id);
        }

        self.logic.start(
            Arc::clone(&self.topology),
            Arc::clone(&self.planets),
            Arc::clone(&self.explorers),
            Arc::clone(&self.prob_registry),
            Arc::clone(&self.planet_rx),
            Arc::clone(&self.explorer_rx),
            Arc::clone(&self.forge),
        )
    }

    /// Stops the autonomous game logic loop and pauses all explorer and planet AIs.
    ///
    /// Planets must actually transition to their stopped state here (not just
    /// have the tick thread go quiet) — a running planet silently ignores a
    /// second `StartPlanetAI` with no ack (see `Planet::handle_orchestrator_msg`),
    /// so without this a later [`Self::start_logic`] would hang waiting for an
    /// ack that never comes.
    ///
    /// # Errors
    /// Returns [`OrchestratorError::InvalidState`] if not running.
    pub fn stop_logic(&mut self) -> Result<(), OrchestratorError> {
        self.logic.stop()?;

        let mut pending_explorers: std::collections::HashSet<ID> = {
            let explorers = self.explorers.lock().unwrap();
            for h in explorers.iter() {
                let _ = h.send(OrchestratorToExplorer::StopExplorerAI);
            }
            explorers.iter().map(|h| h.id()).collect()
        };

        // Wait for these acks now (rather than firing and forgetting) so a
        // leftover StopExplorerAIResult can't later be misread as the
        // StartExplorerAIResult a subsequent `start_logic` is waiting for.
        while !pending_explorers.is_empty() {
            let rx = self.explorer_rx.lock().unwrap();
            let explorer_id = recv_ack(
                &rx,
                API_TIMEOUT,
                "StopExplorerAIResult",
                |msg| match msg {
                    ExplorerToOrchestrator::StopExplorerAIResult { explorer_id }
                        if pending_explorers.contains(&explorer_id) =>
                    {
                        Ok(explorer_id)
                    }
                    other => Err(other),
                },
            )?;
            pending_explorers.remove(&explorer_id);
        }

        let mut pending_planets: std::collections::HashSet<ID> = {
            let planets = self.planets.lock().unwrap();
            for h in planets.iter() {
                h.send(OrchestratorToPlanet::StopPlanetAI)
                    .map_err(OrchestratorError::ChannelError)?;
            }
            planets.iter().map(|h| h.id()).collect()
        };

        while !pending_planets.is_empty() {
            let rx = self.planet_rx.lock().unwrap();
            let planet_id = recv_ack(
                &rx,
                API_TIMEOUT,
                "StopPlanetAIResult",
                |msg| match msg {
                    PlanetToOrchestrator::StopPlanetAIResult { planet_id }
                        if pending_planets.contains(&planet_id) =>
                    {
                        Ok(planet_id)
                    }
                    other => Err(other),
                },
            )?;
            pending_planets.remove(&planet_id);
        }

        Ok(())
    }

    /// Manually sends a sunray to `planet_id` and waits for [`SunrayAck`].
    ///
    /// # Errors
    /// [`OrchestratorError::PlanetNotFound`] or [`OrchestratorError::ChannelError`].
    pub fn send_sunray(&self, planet_id: ID) -> Result<(), OrchestratorError> {
        let sunray = {
            let forge = self.forge.lock().unwrap();
            forge.generate_sunray()
        };
        {
            let planets = self.planets.lock().unwrap();
            let planet = planets
                .get(planet_id)
                .ok_or(OrchestratorError::PlanetNotFound(planet_id))?;
            planet
                .send(OrchestratorToPlanet::Sunray(sunray))
                .map_err(OrchestratorError::ChannelError)?;
        }
        // Record the same events `dispatch_to_planet` records for an
        // autonomous sunray — without this, manual sunrays were invisible to
        // the GUI's whole portrait-reaction system (which only reads
        // `GalaxyEvent`s from the snapshot), so only the client-side-only
        // glitch flash ever showed for a manual action.
        self.prob_registry
            .lock()
            .unwrap()
            .record_event(crate::probability::OrchestratorEvent::SunraySent { planet_id });

        let rx = self.planet_rx.lock().unwrap();
        let result = recv_ack(
            &rx,
            API_TIMEOUT,
            &format!("SunrayAck from planet {planet_id}"),
            |msg| match msg {
                PlanetToOrchestrator::SunrayAck { planet_id: id } if id == planet_id => Ok(()),
                other => Err(other),
            },
        )
        .inspect(|()| log::info!("Manual sunray: SunrayAck from planet {planet_id}"));

        if result.is_ok() {
            self.prob_registry
                .lock()
                .unwrap()
                .record_event(crate::probability::OrchestratorEvent::SunrayReceived { planet_id });
            // "Infiltrating the orchestrator's mind" — a manual sunray nudges
            // the whole galaxy's mood back toward calm, not just this planet.
            self.prob_registry
                .lock()
                .unwrap()
                .nudge_hostility(-crate::probability::MANUAL_OVERRIDE_NUDGE);
        }
        result
    }

    /// Manually sends an asteroid to `planet_id` and handles the result.
    ///
    /// If the planet cannot deflect it, the planet is destroyed.
    ///
    /// # Errors
    /// [`OrchestratorError::PlanetNotFound`] or [`OrchestratorError::ChannelError`].
    pub fn send_asteroid(&mut self, planet_id: ID) -> Result<bool, OrchestratorError> {
        let asteroid = {
            let forge = self.forge.lock().unwrap();
            forge.generate_asteroid()
        };
        {
            let planets = self.planets.lock().unwrap();
            let planet = planets
                .get(planet_id)
                .ok_or(OrchestratorError::PlanetNotFound(planet_id))?;
            planet
                .send(OrchestratorToPlanet::Asteroid(asteroid))
                .map_err(OrchestratorError::ChannelError)?;
        }
        // See `send_sunray` — without this, manual asteroids never showed
        // Solace's "unwanted_event" (guilty) reaction, only the glitch flash.
        self.prob_registry
            .lock()
            .unwrap()
            .record_event(crate::probability::OrchestratorEvent::AsteroidSent { planet_id });

        let rocket = {
            let rx = self.planet_rx.lock().unwrap();
            recv_ack(
                &rx,
                API_TIMEOUT,
                &format!("AsteroidAck from planet {planet_id}"),
                |msg| match msg {
                    PlanetToOrchestrator::AsteroidAck { planet_id: id, rocket } if id == planet_id => {
                        Ok(rocket)
                    }
                    other => Err(other),
                },
            )?
        };

        let survived = rocket.is_some();
        if !survived {
            log::warn!("Planet {planet_id} destroyed by manual asteroid");
            let mut planets = self.planets.lock().unwrap();
            let mut topo = self.topology.lock().unwrap();
            let mut prob = self.prob_registry.lock().unwrap();
            let mut explorers = self.explorers.lock().unwrap();
            crate::logic::tick::destroy_planet(
                planet_id,
                &mut planets,
                &mut topo,
                &mut prob,
                &mut explorers,
                &mut rand::rng(),
            )?;
            // destroy_planet already dampened hostility above; no extra nudge needed.
        } else {
            self.prob_registry
                .lock()
                .unwrap()
                .record_event(crate::probability::OrchestratorEvent::AsteroidDeflected { planet_id });
            // "Infiltrating the orchestrator's mind" — a manual asteroid nudges
            // the whole galaxy's mood toward hostility, not just this planet.
            self.prob_registry
                .lock()
                .unwrap()
                .nudge_hostility(crate::probability::MANUAL_OVERRIDE_NUDGE);
        }
        Ok(survived)
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

    /// Debug-only: directly nudges hostility by `delta` (clamped to
    /// `[0.0, 1.0]`), bypassing the normal sunray/asteroid-driven climb.
    /// Lets the GUI verify the Eclipse/Solace flip instantly instead of
    /// waiting through many real death cycles.
    pub fn debug_nudge_hostility(&self, delta: f64) {
        self.prob_registry.lock().unwrap().nudge_hostility(delta);
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

    /// Returns the content of the explorer bag.
    ///
    /// # Errors
    /// [`OrchestratorError::ExplorerNotFound`] or channel error.
    pub fn bag_content(
        &self,
        explorer_id: ID,
    ) -> Result<crate::explorer::handle::BagContent, OrchestratorError> {
        {
            let explorers = self.explorers.lock().unwrap();
            let h = explorers
                .get(explorer_id)
                .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?;

            h.send(OrchestratorToExplorer::BagContentRequest)
                .map_err(OrchestratorError::ChannelError)?;
        }

        let rx = self.explorer_rx.lock().unwrap();

        match rx.recv_timeout(API_TIMEOUT) {
            Ok(ExplorerToOrchestrator::BagContentResponse {
                   explorer_id: ack_id,
                   bag_content,
               }) if ack_id == explorer_id => Ok(bag_content),

            Ok(other) => Err(OrchestratorError::ChannelError(format!(
                "Expected BagContentResponse from explorer {explorer_id}, got {other:?}"
            ))),

            Err(_) => Err(OrchestratorError::ChannelError(format!(
                "Timeout waiting for BagContentResponse from explorer {explorer_id}"
            ))),
        }
    }
}
