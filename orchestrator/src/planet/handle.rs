//! What the orchestrator keeps for each planet: the sender to its thread and
//! the sender explorers use to talk to it.

use common_game::protocols::orchestrator_planet::OrchestratorToPlanet;
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::Sender;

pub struct PlanetHandle {
    id: ID,
    sender: Sender<OrchestratorToPlanet>,
    // group name, for logs
    label: String,
    // cloned and given to each explorer that moves here
    explorer_tx: Sender<ExplorerToPlanet>,
    // Option so join() can take it
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl PlanetHandle {
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

    #[must_use]
    pub fn id(&self) -> ID {
        self.id
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// # Errors
    /// If the planet thread is gone.
    pub fn send(&self, msg: OrchestratorToPlanet) -> Result<(), String> {
        self.sender
            .send(msg)
            .map_err(|_| format!("Planet {} disconnected", self.id))
    }

    #[must_use]
    pub fn explorer_sender(&self) -> Sender<ExplorerToPlanet> {
        self.explorer_tx.clone()
    }

    /// Send `KillPlanet` first, otherwise this waits forever.
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take()
            && let Err(e) = handle.join()
        {
            log::error!("Planet {} thread panicked: {e:?}", self.id);
        }
    }
}
