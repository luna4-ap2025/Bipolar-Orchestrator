//! Which planets are alive and who they're connected to. Destroyed planets
//! get removed.

use common_game::utils::ID;
use std::collections::{HashMap, HashSet};

pub struct Topology {
    // planet -> alive neighbors
    adjacency: HashMap<ID, HashSet<ID>>,
}

impl Topology {
    pub(super) fn from_adjacency(adjacency: HashMap<ID, HashSet<ID>>) -> Self {
        Self { adjacency }
    }

    /// Ids of the planets that are still alive.
    pub fn planet_ids(&self) -> impl Iterator<Item = ID> + '_ {
        self.adjacency.keys().copied()
    }

    #[must_use]
    pub fn contains(&self, id: ID) -> bool {
        self.adjacency.contains_key(&id)
    }

    /// Empty if the planet doesn't exist.
    #[must_use]
    pub fn neighbors(&self, planet_id: ID) -> Vec<ID> {
        self.adjacency
            .get(&planet_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn are_neighbors(&self, a: ID, b: ID) -> bool {
        self.adjacency.get(&a).is_some_and(|s| s.contains(&b))
    }

    /// Removes the planet and all its connections. Does nothing if it's
    /// already gone.
    pub fn remove_planet(&mut self, planet_id: ID) {
        if let Some(neighbors) = self.adjacency.remove(&planet_id) {
            for neighbor in neighbors {
                if let Some(neighbor_set) = self.adjacency.get_mut(&neighbor) {
                    neighbor_set.remove(&planet_id);
                }
            }
        }
    }

    #[must_use]
    pub fn planet_count(&self) -> usize {
        self.adjacency.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn simple_topology() -> Topology {
        // 1-2-3-1 ring
        let mut adj = HashMap::new();
        adj.insert(1, HashSet::from([2, 3]));
        adj.insert(2, HashSet::from([1, 3]));
        adj.insert(3, HashSet::from([1, 2]));
        Topology::from_adjacency(adj)
    }

    #[test]
    fn neighbors_returns_correct_set() {
        let t = simple_topology();
        let mut n = t.neighbors(1);
        n.sort_unstable();
        assert_eq!(n, vec![2, 3]);
    }

    #[test]
    fn remove_planet_updates_neighbors() {
        let mut t = simple_topology();
        t.remove_planet(1);

        assert!(!t.contains(1));
        assert!(!t.neighbors(2).contains(&1));
        assert!(!t.neighbors(3).contains(&1));
        assert_eq!(t.planet_count(), 2);
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let mut t = simple_topology();
        t.remove_planet(99);
        assert_eq!(t.planet_count(), 3);
    }

    #[test]
    fn are_neighbors_is_symmetric() {
        let t = simple_topology();
        assert!(t.are_neighbors(1, 2));
        assert!(t.are_neighbors(2, 1));
    }
}
