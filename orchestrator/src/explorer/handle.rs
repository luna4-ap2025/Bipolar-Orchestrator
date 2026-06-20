//! # Explorer handle
//!
//! [`ExplorerHandle`] is the orchestrator's bookkeeping for one live explorer.
//! It holds the sender to the explorer thread and tracks which planet the
//! explorer is currently on.
//!
//! ## Owner: Vivi

use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};

/// Type alias for the concrete `ExplorerToOrchestrator` message used by the orchestrator.
/// The bag content is `Vec<String>` (resource names as strings).
pub type ExplorerToOrchestratorMsg = ExplorerToOrchestrator<Vec<String>>;
use common_game::utils::ID;
use crossbeam_channel::Sender;

/// The bag content type the orchestrator uses. Since the orchestrator does not
/// inspect bag contents beyond forwarding them to the visualizer, we store them
/// as a `Vec<String>` (resource names). Adjust if your explorer sends a richer type.
pub type BagContent = Vec<String>;

/// The orchestrator's bookkeeping for one live explorer.
pub struct ExplorerHandle {
    /// Unique identifier of this explorer.
    id: ID,
    /// Sender used to deliver messages to the explorer thread.
    sender: Sender<OrchestratorToExplorer>,
    /// The ID of the planet the explorer is currently on.
    /// Updated by the router after every successful move.
    current_planet: ID,
    /// Join handle for the explorer thread.
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl ExplorerHandle {
    /// Constructs a new `ExplorerHandle`.
    pub fn new(
        id: ID,
        sender: Sender<OrchestratorToExplorer>,
        starting_planet: ID,
        thread_handle: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            id,
            sender,
            current_planet: starting_planet,
            thread_handle: Some(thread_handle),
        }
    }

    /// Returns the explorer's unique ID.
    pub fn id(&self) -> ID {
        self.id
    }

    /// Returns the ID of the planet the explorer is currently on.
    pub fn current_planet(&self) -> ID {
        self.current_planet
    }

    /// Updates the current planet ID after a successful move.
    pub fn set_current_planet(&mut self, planet_id: ID) {
        self.current_planet = planet_id;
    }

    /// Sends a message to the explorer thread.
    ///
    /// # Errors
    /// Returns an error string if the explorer thread has disconnected.
    pub fn send(&self, msg: OrchestratorToExplorer) -> Result<(), String> {
        self.sender
            .send(msg)
            .map_err(|_| format!("Explorer {} disconnected", self.id))
    }

    /// Waits for the explorer thread to finish.
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            if let Err(e) = handle.join() {
                log::error!("Explorer {} thread panicked: {e:?}", self.id);
            }
        }
    }
}
