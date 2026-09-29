//! The explorers that are still alive.

pub mod handle;

pub use handle::ExplorerHandle;

use common_game::utils::ID;
use std::collections::HashMap;

pub struct ExplorerRegistry {
    handles: HashMap<ID, ExplorerHandle>,
}

impl ExplorerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    pub fn insert(&mut self, handle: ExplorerHandle) {
        self.handles.insert(handle.id(), handle);
    }

    #[must_use]
    pub fn get(&self, explorer_id: ID) -> Option<&ExplorerHandle> {
        self.handles.get(&explorer_id)
    }

    pub fn get_mut(&mut self, explorer_id: ID) -> Option<&mut ExplorerHandle> {
        self.handles.get_mut(&explorer_id)
    }

    pub fn remove(&mut self, explorer_id: ID) -> Option<ExplorerHandle> {
        self.handles.remove(&explorer_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ExplorerHandle> {
        self.handles.values()
    }

    /// No explorers left means the game is over (spec 1.3).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

impl Default for ExplorerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;
    use common_game::protocols::planet_explorer::PlanetToExplorer;
    use crossbeam_channel::unbounded;

    fn fake_explorer(id: ID) -> ExplorerHandle {
        let (tx, _rx) = unbounded::<OrchestratorToExplorer>();
        let (planet_reply_tx, _rx2) = unbounded::<PlanetToExplorer>();
        let thread = std::thread::spawn(|| {});
        ExplorerHandle::new(id, tx, planet_reply_tx, 1, thread)
    }

    #[test]
    fn is_empty_reflects_registry_contents() {
        let mut reg = ExplorerRegistry::new();
        assert!(reg.is_empty());

        reg.insert(fake_explorer(1));
        assert!(!reg.is_empty());

        reg.remove(1);
        assert!(reg.is_empty());
    }
}
