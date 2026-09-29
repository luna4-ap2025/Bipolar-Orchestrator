//! The orchestrator API. As the spec asks, every method is synchronous: it
//! waits for the ack before returning. Used by the CLI and by the GUI.

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
    ExplorerToOrchestrator, OrchestratorToExplorer,
};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::utils::ID;
use crossbeam_channel::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const API_TIMEOUT: Duration = Duration::from_secs(10);

/// Owns all the game state. Built once, after the planets and explorers are
/// spawned (see `builder.rs`).
pub struct OrchestratorApi {
    pub(crate) topology: Arc<Mutex<Topology>>,
    pub(crate) planets: Arc<Mutex<PlanetRegistry>>,
    pub(crate) explorers: Arc<Mutex<ExplorerRegistry>>,
    pub(crate) prob_registry: Arc<Mutex<ProbabilityRegistry>>,
    // one receiver shared by all planets, and one by all explorers
    pub(crate) planet_rx: Arc<Mutex<Receiver<PlanetToOrchestrator>>>,
    pub(crate) explorer_rx:
        Arc<Mutex<Receiver<crate::explorer::handle::ExplorerToOrchestratorMsg>>>,
    // makes the sunrays and asteroids
    pub(crate) forge: Arc<Mutex<Forge>>,
    pub(crate) logic: LogicController,
}

impl OrchestratorApi {
    #[must_use]
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

    /// Starts the explorer and planet AIs and the logic loop (sunrays and
    /// asteroids every tick).
    ///
    /// # Errors
    /// `InvalidState` if it's already running, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
    pub fn start_logic(&mut self) -> Result<(), OrchestratorError> {
        {
            let explorers = self.explorers.lock().unwrap();
            for h in explorers.iter() {
                h.send(OrchestratorToExplorer::StartExplorerAI)
                    .map_err(OrchestratorError::ChannelError)?;
            }
        }

        {
            let planets = self.planets.lock().unwrap();
            for h in planets.iter() {
                h.send(OrchestratorToPlanet::StartPlanetAI)
                    .map_err(OrchestratorError::ChannelError)?;
            }
        }

        // wait for every planet's ack (recv_ack skips unrelated messages)
        let mut pending_planets: std::collections::HashSet<ID> = {
            let planets = self.planets.lock().unwrap();
            planets
                .iter()
                .map(super::planet::handle::PlanetHandle::id)
                .collect()
        };

        while !pending_planets.is_empty() {
            let rx = self.planet_rx.lock().unwrap();
            let planet_id = recv_ack(&rx, API_TIMEOUT, "StartPlanetAIResult", |msg| match msg {
                PlanetToOrchestrator::StartPlanetAIResult { planet_id }
                    if pending_planets.contains(&planet_id) =>
                {
                    Ok(planet_id)
                }
                other => Err(other),
            })?;
            pending_planets.remove(&planet_id);
        }

        // same for explorers (there can be a leftover Stop ack from a pause)
        let mut pending_explorers: std::collections::HashSet<ID> = {
            let explorers = self.explorers.lock().unwrap();
            explorers
                .iter()
                .map(super::explorer::handle::ExplorerHandle::id)
                .collect()
        };

        while !pending_explorers.is_empty() {
            let rx = self.explorer_rx.lock().unwrap();
            let explorer_id =
                recv_ack(&rx, API_TIMEOUT, "StartExplorerAIResult", |msg| match msg {
                    ExplorerToOrchestrator::StartExplorerAIResult { explorer_id }
                        if pending_explorers.contains(&explorer_id) =>
                    {
                        Ok(explorer_id)
                    }
                    other => Err(other),
                })?;
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

    /// Stops the logic loop and the explorer and planet AIs.
    ///
    /// The planets really have to be stopped: a planet that's still running
    /// ignores `StartPlanetAI` without sending an ack, so the next
    /// `start_logic` would hang.
    ///
    /// # Errors
    /// `InvalidState` if it isn't running, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
    pub fn stop_logic(&mut self) -> Result<(), OrchestratorError> {
        self.logic.stop()?;

        let mut pending_explorers: std::collections::HashSet<ID> = {
            let explorers = self.explorers.lock().unwrap();
            for h in explorers.iter() {
                let _ = h.send(OrchestratorToExplorer::StopExplorerAI);
            }
            explorers
                .iter()
                .map(super::explorer::handle::ExplorerHandle::id)
                .collect()
        };

        // wait for them now so they don't get in the way of the next start_logic
        while !pending_explorers.is_empty() {
            let rx = self.explorer_rx.lock().unwrap();
            let explorer_id =
                recv_ack(&rx, API_TIMEOUT, "StopExplorerAIResult", |msg| match msg {
                    ExplorerToOrchestrator::StopExplorerAIResult { explorer_id }
                        if pending_explorers.contains(&explorer_id) =>
                    {
                        Ok(explorer_id)
                    }
                    other => Err(other),
                })?;
            pending_explorers.remove(&explorer_id);
        }

        let mut pending_planets: std::collections::HashSet<ID> = {
            let planets = self.planets.lock().unwrap();
            for h in planets.iter() {
                h.send(OrchestratorToPlanet::StopPlanetAI)
                    .map_err(OrchestratorError::ChannelError)?;
            }
            planets
                .iter()
                .map(super::planet::handle::PlanetHandle::id)
                .collect()
        };

        while !pending_planets.is_empty() {
            let rx = self.planet_rx.lock().unwrap();
            let planet_id = recv_ack(&rx, API_TIMEOUT, "StopPlanetAIResult", |msg| match msg {
                PlanetToOrchestrator::StopPlanetAIResult { planet_id }
                    if pending_planets.contains(&planet_id) =>
                {
                    Ok(planet_id)
                }
                other => Err(other),
            })?;
            pending_planets.remove(&planet_id);
        }

        Ok(())
    }

    /// Sends a sunray to `planet_id` and waits for the `SunrayAck`.
    ///
    /// # Errors
    /// `PlanetNotFound`, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
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
        // same events as the automatic sunrays, so the GUI reacts to manual
        // ones too
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
            // the player messing with her decisions changes her mood
            self.prob_registry
                .lock()
                .unwrap()
                .nudge_hostility(-crate::probability::MANUAL_OVERRIDE_NUDGE);
        }
        result
    }

    /// Sends an asteroid to `planet_id`. Returns whether the planet survived;
    /// if it had no rocket it gets destroyed.
    ///
    /// # Errors
    /// `PlanetNotFound`, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
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
        // see send_sunray
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
                    PlanetToOrchestrator::AsteroidAck {
                        planet_id: id,
                        rocket,
                    } if id == planet_id => Ok(rocket),
                    other => Err(other),
                },
            )?
        };

        let survived = rocket.is_some();
        if survived {
            self.prob_registry.lock().unwrap().record_event(
                crate::probability::OrchestratorEvent::AsteroidDeflected { planet_id },
            );
            self.prob_registry
                .lock()
                .unwrap()
                .nudge_hostility(crate::probability::MANUAL_OVERRIDE_NUDGE);
        } else {
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
            // no nudge here, destroy_planet already lowers hostility
        }
        Ok(survived)
    }

    /// Moves `explorer_id` to `dst_planet_id`, which has to be a neighbor of
    /// the planet it's on.
    ///
    /// # Errors
    /// See [`crate::routing::move_explorer`].
    ///
    /// # Panics
    /// If a mutex is poisoned.
    pub fn move_explorer(
        &self,
        explorer_id: ID,
        dst_planet_id: ID,
    ) -> Result<(), OrchestratorError> {
        let current_planet_id = {
            let explorers = self.explorers.lock().unwrap();
            explorers
                .get(explorer_id)
                .ok_or(OrchestratorError::ExplorerNotFound(explorer_id))?
                .current_planet()
        };

        // explorer_rx is held too, so the logic thread doesn't handle explorer
        // messages in the middle of the move
        let planet_rx = self.planet_rx.lock().unwrap();
        let _explorer_rx = self.explorer_rx.lock().unwrap();

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

    /// Asks the explorer to generate a basic resource on its planet.
    ///
    /// # Errors
    /// `ExplorerNotFound`, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
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

        let generated = recv_ack(
            &rx,
            API_TIMEOUT,
            &format!("GenerateResourceResponse from explorer {explorer_id}"),
            |msg| match msg {
                ExplorerToOrchestrator::GenerateResourceResponse {
                    explorer_id: ack_id,
                    generated,
                } if ack_id == explorer_id => Ok(generated),
                other => Err(other),
            },
        )?;

        generated.map_err(OrchestratorError::ChannelError)
    }

    /// Asks the explorer to make a complex resource on its planet.
    ///
    /// # Errors
    /// `ExplorerNotFound`, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
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

        let generated = recv_ack(
            &rx,
            API_TIMEOUT,
            &format!("CombineResourceResponse from explorer {explorer_id}"),
            |msg| match msg {
                ExplorerToOrchestrator::CombineResourceResponse {
                    explorer_id: ack_id,
                    generated,
                } if ack_id == explorer_id => Ok(generated),
                other => Err(other),
            },
        )?;

        generated.map_err(OrchestratorError::ChannelError)
    }

    /// Energy cells and rocket of `planet_id` (used by the GUI holograms).
    ///
    /// # Errors
    /// `PlanetNotFound`, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
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
        recv_ack(
            &rx,
            API_TIMEOUT,
            &format!("InternalStateResponse from planet {planet_id}"),
            |msg| match msg {
                PlanetToOrchestrator::InternalStateResponse { planet_state, .. } => {
                    Ok(planet_state)
                }
                other => Err(other),
            },
        )
    }

    /// # Errors
    /// `PlanetNotFound` if the planet doesn't exist (or was destroyed).
    ///
    /// # Panics
    /// If the topology mutex is poisoned.
    pub fn neighbors(&self, planet_id: ID) -> Result<Vec<ID>, OrchestratorError> {
        let topo = self.topology.lock().unwrap();
        if !topo.contains(planet_id) {
            return Err(OrchestratorError::PlanetNotFound(planet_id));
        }
        Ok(topo.neighbors(planet_id))
    }

    /// # Panics
    /// If the topology mutex is poisoned.
    #[must_use]
    pub fn alive_planets(&self) -> Vec<ID> {
        self.topology.lock().unwrap().planet_ids().collect()
    }

    /// Debug only (E/S keys in the GUI), to test the Solace/Eclipse flip
    /// without playing a whole game.
    ///
    /// # Panics
    /// If the mutex is poisoned.
    pub fn debug_nudge_hostility(&self, delta: f64) {
        self.prob_registry.lock().unwrap().nudge_hostility(delta);
    }

    /// Stops the logic and kills every planet and explorer thread.
    ///
    /// # Panics
    /// If a mutex is poisoned.
    pub fn shutdown(&mut self) {
        if self.logic.is_running() {
            let _ = self.logic.stop();
        }

        let planet_ids: Vec<_> = {
            let planets = self.planets.lock().unwrap();
            planets
                .iter()
                .map(super::planet::handle::PlanetHandle::id)
                .collect()
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

        let explorer_ids: Vec<_> = {
            let explorers = self.explorers.lock().unwrap();
            explorers
                .iter()
                .map(super::explorer::handle::ExplorerHandle::id)
                .collect()
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

    /// # Errors
    /// `ExplorerNotFound`, or a channel/timeout error.
    ///
    /// # Panics
    /// If a mutex is poisoned.
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

        recv_ack(
            &rx,
            API_TIMEOUT,
            &format!("BagContentResponse from explorer {explorer_id}"),
            |msg| match msg {
                ExplorerToOrchestrator::BagContentResponse {
                    explorer_id: ack_id,
                    bag_content,
                } if ack_id == explorer_id => Ok(bag_content),
                other => Err(other),
            },
        )
    }
}
