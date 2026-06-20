//! # Planet spawner
//!
//! Creates channels, calls the correct planet factory, and launches the planet
//! thread. Returns a [`PlanetHandle`] to the orchestrator.
//!
//! ## Owner: Vivi

use super::factories::PlanetFactory;
use super::PlanetHandle;
use crate::error::OrchestratorError;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender, unbounded};

/// All the channel ends the orchestrator keeps after spawning a planet.
pub struct SpawnedPlanetChannels {
    /// Receiver for all messages coming from all planets (shared single receiver).
    /// The orchestrator already owns this; this field is not included here.
    /// Instead the planet's *sender* half is given to the planet thread.
    ///
    /// This struct only carries the PlanetToOrchestrator sender so the planet
    /// thread can send back to the orchestrator.
    ///
    /// Actually see design note below.
    _note: (),
}

/// Spawns a planet by:
/// 1. Creating the orchestrator↔planet channels.
/// 2. Creating the explorer→planet channel (one shared receiver per planet).
/// 3. Calling the correct factory function (dispatched by `factory`).
/// 4. Launching a thread that calls `planet.run()`.
/// 5. Returning a [`PlanetHandle`] to the caller.
///
/// The `orch_tx` parameter is the **shared** sender used by **all** planets to
/// send messages back to the orchestrator (the orchestrator owns the single receiver).
///
/// # Errors
/// Returns [`OrchestratorError::PlanetConstructionError`] if the factory fails.
pub fn spawn_planet(
    id: ID,
    label: impl Into<String>,
    factory: &dyn PlanetFactory,
    orch_tx: Sender<PlanetToOrchestrator>,
) -> Result<PlanetHandle, OrchestratorError> {
    let label = label.into();

    // one dedicated channel per planet so we can target it specifically
    let (tx_to_planet, rx_from_orch) = unbounded();

    // all explorers currently on this planet share one sender to reach it
    // we clone tx_explorer_to_planet and hand the clone to each explorer that visits
    let (tx_explorer_to_planet, rx_from_explorers) = unbounded();

    // call the planet group's factory to get the Planet struct
    let mut planet = factory
        .create(id, rx_from_orch, orch_tx, rx_from_explorers)
        .map_err(|e| OrchestratorError::PlanetConstructionError(e))?;

    // spawn the planet in its own thread
    let label_clone = label.clone();
    let thread_handle = std::thread::Builder::new()
        .name(format!("planet-{id}-{label_clone}"))
        .spawn(move || {
            log::info!("Planet {id} ({label_clone}) thread started");
            match planet.run() {
                Ok(()) => log::info!("Planet {id} ({label_clone}) thread finished cleanly"),
                Err(e) => log::error!("Planet {id} ({label_clone}) thread error: {e}"),
            }
        })
        .map_err(|e| OrchestratorError::PlanetConstructionError(e.to_string()))?;

    Ok(PlanetHandle::new(
        id,
        label,
        tx_to_planet,
        tx_explorer_to_planet,
        thread_handle,
    ))
}
