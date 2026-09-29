//! What the orchestrator keeps for each explorer: the sender to its thread
//! and the planet it's on.

use common_game::components::resource::ResourceType;
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator, OrchestratorToExplorer,
};
use common_game::protocols::planet_explorer::PlanetToExplorer;
use common_game::utils::ID;
use crossbeam_channel::Sender;

/// Resource type and how many, the same type our explorers send in
/// `BagContentResponse`.
pub type BagContent = Vec<(ResourceType, usize)>;

pub type ExplorerToOrchestratorMsg = ExplorerToOrchestrator<BagContent>;

pub struct ExplorerHandle {
    id: ID,
    sender: Sender<OrchestratorToExplorer>,

    // Planets use this to answer the explorer. It's made once at spawn and
    // passed to each new planet when the explorer moves.
    planet_reply_tx: Sender<PlanetToExplorer>,

    // updated after every move
    current_planet: ID,

    // Last bag the logic loop received. It's stored instead of asked for,
    // because waiting for an answer on the shared explorer channel could eat
    // messages meant for the logic loop (this happened when the GUI called
    // bag_content every snapshot).
    last_bag: BagContent,

    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl ExplorerHandle {
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

    #[must_use]
    pub fn id(&self) -> ID {
        self.id
    }

    #[must_use]
    pub fn current_planet(&self) -> ID {
        self.current_planet
    }

    pub fn set_current_planet(&mut self, planet_id: ID) {
        self.current_planet = planet_id;
    }

    /// Empty until the first `BagContentResponse` arrives.
    #[must_use]
    pub fn bag(&self) -> &BagContent {
        &self.last_bag
    }

    /// Only called by the logic loop when it reads a `BagContentResponse`.
    pub fn set_bag(&mut self, bag: BagContent) {
        self.last_bag = bag;
    }

    #[must_use]
    pub fn planet_reply_tx(&self) -> Sender<PlanetToExplorer> {
        self.planet_reply_tx.clone()
    }

    /// # Errors
    /// If the explorer thread is gone.
    pub fn send(&self, msg: OrchestratorToExplorer) -> Result<(), String> {
        self.sender
            .send(msg)
            .map_err(|_| format!("Explorer {} disconnected", self.id))
    }

    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take()
            && let Err(e) = handle.join()
        {
            log::error!("Explorer {} thread panicked: {e:?}", self.id);
        }
    }
}
