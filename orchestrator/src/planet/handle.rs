//! # Planet handle
//!
//! [`PlanetHandle`] is the orchestrator's view of a live planet.
//! It holds the sender used to send messages to the planet thread and
//! any metadata the orchestrator needs to track per planet.
//!
//! ## Owner: Vivi

use common_game::protocols::orchestrator_planet::OrchestratorToPlanet;
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::Sender;

/// The orchestrator's bookkeeping for one live planet.
pub struct PlanetHandle {
    /// Stable identifier of this planet.
    id: ID,
    /// Sender used to deliver messages to the planet thread.
    /// There is one dedicated sender per planet (the planet owns the receiver).
    sender: Sender<OrchestratorToPlanet>,
    /// Name/label for display (e.g. the group name).
    label: String,
    /// The single shared receiver-end sender for explorers → planet.
    ///
    /// When the orchestrator moves an explorer onto this planet it clones this
    /// sender and passes it to the explorer via [`OrchestratorToExplorer::MoveToPlanet`].
    explorer_tx: Sender<ExplorerToPlanet>,
    /// Join handle for the planet thread, used during shutdown.
    ///
    /// Wrapped in `Option` so we can `take()` it once during `join`.
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl PlanetHandle {
    /// Constructs a new `PlanetHandle`.
    ///
    /// Called by [`super::spawner::spawn_planet`] after the planet thread is started.
    pub fn new(
        id: ID,
        label: impl Into<String>,
        sender: Sender<OrchestratorToPlanet>,
        explorer_tx: Sender<ExplorerToPlanet>,
        thread_handle: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            sender,
            explorer_tx,
            thread_handle: Some(thread_handle),
        }
    }

    /// Returns the planet's unique ID.
    pub fn id(&self) -> ID {
        self.id
    }

    /// Returns the planet's display label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Sends a message to the planet thread.
    ///
    /// # Errors
    /// Returns an error string if the planet thread has disconnected.
    pub fn send(&self, msg: OrchestratorToPlanet) -> Result<(), String> {
        self.sender
            .send(msg)
            .map_err(|_| format!("Planet {} disconnected", self.id))
    }

    /// Returns a clone of the sender used to deliver explorer messages to this
    /// planet. Given to an explorer when it moves to this planet.
    pub fn explorer_sender(&self) -> Sender<ExplorerToPlanet> {
        self.explorer_tx.clone()
    }

    /// Waits for the planet thread to finish.
    ///
    /// Should be called after sending [`OrchestratorToPlanet::KillPlanet`].
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            if let Err(e) = handle.join() {
                log::error!("Planet {} thread panicked: {e:?}", self.id);
            }
        }
    }
}
