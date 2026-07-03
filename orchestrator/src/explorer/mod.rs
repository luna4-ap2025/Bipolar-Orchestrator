//! # Explorer manager
//!
//! Owns the live set of [`ExplorerHandle`]s.
//!
//! ## Owner: Vivi

pub mod handle;

pub use handle::ExplorerHandle;

use common_game::utils::ID;
use std::collections::HashMap;

/// Owns all currently alive explorer handles.
pub struct ExplorerRegistry {
    handles: HashMap<ID, ExplorerHandle>,
}

impl ExplorerRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    /// Inserts a newly spawned explorer handle.
    pub fn insert(&mut self, handle: ExplorerHandle) {
        self.handles.insert(handle.id(), handle);
    }

    /// Returns an immutable reference to the handle for `explorer_id`.
    #[must_use]
    pub fn get(&self, explorer_id: ID) -> Option<&ExplorerHandle> {
        self.handles.get(&explorer_id)
    }

    /// Returns a mutable reference to the handle for `explorer_id`.
    pub fn get_mut(&mut self, explorer_id: ID) -> Option<&mut ExplorerHandle> {
        self.handles.get_mut(&explorer_id)
    }

    /// Removes and returns the handle for `explorer_id`.
    pub fn remove(&mut self, explorer_id: ID) -> Option<ExplorerHandle> {
        self.handles.remove(&explorer_id)
    }

    /// Iterates over all live explorer handles.
    pub fn iter(&self) -> impl Iterator<Item = &ExplorerHandle> {
        self.handles.values()
    }

    /// Returns `true` if no explorers remain — the game's end condition
    /// (spec §1.3: "when no Explorer remains, the game ends").
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
