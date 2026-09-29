//! Creates a planet's channels, builds it with its group's factory and runs
//! it in its own thread.

use super::PlanetHandle;
use super::factories::PlanetFactory;
use crate::error::OrchestratorError;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;
use common_game::utils::ID;
use crossbeam_channel::{Sender, unbounded};

// TODO: not used anywhere, remove
pub struct SpawnedPlanetChannels {
    _note: (),
}

/// `orch_tx` is the sender all planets share to talk to the orchestrator.
///
/// # Errors
/// `PlanetConstructionError` if the factory fails or the thread can't start.
pub fn spawn_planet(
    id: ID,
    label: impl Into<String>,
    factory: &dyn PlanetFactory,
    orch_tx: Sender<PlanetToOrchestrator>,
) -> Result<PlanetHandle, OrchestratorError> {
    let label = label.into();

    // one channel per planet so we can send to a specific one
    let (tx_to_planet, rx_from_orch) = unbounded();

    // explorers get a clone of this sender when they move here
    let (tx_explorer_to_planet, rx_from_explorers) = unbounded();

    let mut planet = factory
        .create(id, rx_from_orch, orch_tx, rx_from_explorers)
        .map_err(OrchestratorError::PlanetConstructionError)?;

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
