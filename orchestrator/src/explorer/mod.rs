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
}

impl Default for ExplorerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
