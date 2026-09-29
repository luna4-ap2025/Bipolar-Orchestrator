//! The planets that are still alive, plus spawning them and the factories
//! for each group's planet.

pub mod factories;
pub mod handle;
pub mod spawner;

pub use handle::PlanetHandle;
pub use spawner::spawn_planet;

use common_game::utils::ID;
use std::collections::HashMap;

pub struct PlanetRegistry {
    handles: HashMap<ID, PlanetHandle>,
}

impl PlanetRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    pub fn insert(&mut self, handle: PlanetHandle) {
        self.handles.insert(handle.id(), handle);
    }

    #[must_use]
    pub fn get(&self, planet_id: ID) -> Option<&PlanetHandle> {
        self.handles.get(&planet_id)
    }

    pub fn remove(&mut self, planet_id: ID) -> Option<PlanetHandle> {
        self.handles.remove(&planet_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PlanetHandle> {
        self.handles.values()
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.handles.len()
    }
}

impl Default for PlanetRegistry {
    fn default() -> Self {
        Self::new()
    }
}
