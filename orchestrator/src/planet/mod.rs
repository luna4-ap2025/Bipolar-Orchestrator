//! # Planet manager
//!
//! Holds the live set of [`PlanetHandle`]s and the factory dispatch.
//!
//! ## Owner: Vivi (handle + spawner) / Vale (factories)

pub mod factories;
pub mod handle;
pub mod spawner;

pub use handle::PlanetHandle;
pub use spawner::spawn_planet;

use common_game::utils::ID;
use std::collections::HashMap;

/// Owns all currently alive planet handles.
pub struct PlanetRegistry {
    handles: HashMap<ID, PlanetHandle>,
}

impl PlanetRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    /// Inserts a newly spawned planet handle.
    pub fn insert(&mut self, handle: PlanetHandle) {
        self.handles.insert(handle.id(), handle);
    }

    /// Returns an immutable reference to the handle for `planet_id`.
    pub fn get(&self, planet_id: ID) -> Option<&PlanetHandle> {
        self.handles.get(&planet_id)
    }

    /// Removes and returns the handle for `planet_id` (called when it is killed).
    pub fn remove(&mut self, planet_id: ID) -> Option<PlanetHandle> {
        self.handles.remove(&planet_id)
    }

    /// Iterates over all live planet handles.
    pub fn iter(&self) -> impl Iterator<Item = &PlanetHandle> {
        self.handles.values()
    }

    /// Returns the number of alive planets.
    pub fn count(&self) -> usize {
        self.handles.len()
    }
}

impl Default for PlanetRegistry {
    fn default() -> Self {
        Self::new()
    }
}
