//! # Explorer handle
//!
//! [`ExplorerHandle`] is the orchestrator's bookkeeping for one live explorer.
//! It holds the sender to the explorer thread and tracks which planet the
//! explorer is currently on.
//!
//! ## Owner: Vivi

use common_game::components::resource::ResourceType;
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator, OrchestratorToExplorer,
};
use common_game::protocols::planet_explorer::PlanetToExplorer;
use common_game::utils::ID;
use crossbeam_channel::Sender;

/// The bag content type used by the orchestrator.
///
/// It must match the explorer's `BagContentResponse` type.
/// The explorer sends a summary of resource types and quantities:
/// `Vec<(ResourceType, usize)>`.
pub type BagContent = Vec<(ResourceType, usize)>;

/// Type alias for the concrete `ExplorerToOrchestrator` message used by the orchestrator.
pub type ExplorerToOrchestratorMsg = ExplorerToOrchestrator<BagContent>;

/// The orchestrator's bookkeeping for one live explorer.
pub struct ExplorerHandle {
    /// Unique identifier of this explorer.
    id: ID,

    /// Sender used to deliver messages to the explorer thread.
    sender: Sender<OrchestratorToExplorer>,

    /// Sender used by planets to reply directly to this explorer.
    ///
    /// This sender is created once when the explorer is spawned.
    /// The explorer keeps the matching `Receiver<PlanetToExplorer>` permanently.
    planet_reply_tx: Sender<PlanetToExplorer>,

    /// The ID of the planet the explorer is currently on.
    /// Updated by the router after every successful move.
    current_planet: ID,

    /// Most recent `BagContentResponse` the logic loop's regular drain has
    /// seen for this explorer. Populated asynchronously (see
    /// `logic::event_handler::handle_one`) rather than fetched with a
    /// blocking round-trip: a blocking wait on the shared explorer channel
    /// would risk swallowing a real autonomous message (`NeighborsRequest`,
    /// `TravelToPlanetRequest`) meant for the logic loop, which is exactly
    /// what happened when the GUI's per-poll snapshot used to call
    /// `OrchestratorApi::bag_content` directly.
    last_bag: BagContent,

    /// Join handle for the explorer thread.
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl ExplorerHandle {
    /// Constructs a new `ExplorerHandle`.
    #[must_use]
    pub fn new(
        id: ID,
        sender: Sender<OrchestratorToExplorer>,
        planet_reply_tx: Sender<PlanetToExplorer>,
        starting_planet: ID,
        thread_handle: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            id,
            sender,
            planet_reply_tx,
            current_planet: starting_planet,
            last_bag: Vec::new(),
            thread_handle: Some(thread_handle),
        }
    }

    /// Returns the explorer's unique ID.
    #[must_use]
    pub fn id(&self) -> ID {
        self.id
    }

    /// Returns the ID of the planet the explorer is currently on.
    #[must_use]
    pub fn current_planet(&self) -> ID {
        self.current_planet
    }

    /// Updates the current planet ID after a successful move.
    pub fn set_current_planet(&mut self, planet_id: ID) {
        self.current_planet = planet_id;
    }

    /// Returns the last `BagContentResponse` the logic loop's drain has
    /// recorded for this explorer (empty until the first one arrives).
    #[must_use]
    pub fn bag(&self) -> &BagContent {
        &self.last_bag
    }

    /// Records a fresh `BagContentResponse`, called only from the logic
    /// loop's own drain of `explorer_rx` — never from a second concurrent
    /// reader of that channel.
    pub fn set_bag(&mut self, bag: BagContent) {
        self.last_bag = bag;
    }

    /// Returns the sender that planets use to reply to this explorer.
    #[must_use]
    pub fn planet_reply_tx(&self) -> Sender<PlanetToExplorer> {
        self.planet_reply_tx.clone()
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
        if let Some(handle) = self.thread_handle.take()
            && let Err(e) = handle.join()
        {
            log::error!("Explorer {} thread panicked: {e:?}", self.id);
        }
    }
}
