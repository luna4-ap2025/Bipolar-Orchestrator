// Types the orchestrator sends to the GUI. Kept in their own crate so the GUI
// doesn't need to depend on common-game.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Personality {
    #[default]
    Solace,
    Eclipse,
}

// Copy of common_game's ResourceType, snapshot::build converts between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Oxygen,
    Hydrogen,
    Carbon,
    Silicon,
    Diamond,
    Water,
    Life,
    Robot,
    Dolphin,
}

#[derive(Clone, Debug)]
pub struct ExplorerSnapshot {
    pub id: u32,
    pub planet: u32,
    pub bag: Vec<(ResourceKind, usize)>,
}

#[derive(Clone, Debug)]
pub enum GalaxyEvent {
    SunraySent {
        planet_id: u32,
    },
    SunrayReceived {
        planet_id: u32,
    },
    AsteroidSent {
        planet_id: u32,
    },
    AsteroidDeflected {
        planet_id: u32,
    },
    PlanetDestroyed {
        planet_id: u32,
    },
    ExplorerKilled {
        explorer_id: u32,
    },
    ExplorerMoved {
        explorer_id: u32,
        from: u32,
        to: u32,
    },
}

// Built by the bridge thread about 4 times a second.
#[derive(Clone, Debug, Default)]
pub struct GalaxySnapshot {
    pub personality: Personality,
    // 0.0 - 1.0, drives the Solace/Eclipse crossfade
    pub hostility: f64,
    pub phase_elapsed: f64,
    pub alive_planets: Vec<u32>,
    pub explorers: Vec<ExplorerSnapshot>,
    // events since the last snapshot, the GUI takes them out when it reads
    pub events: Vec<GalaxyEvent>,
    // planet id -> alive neighbors (shrinks when planets die)
    pub neighbors: std::collections::HashMap<u32, Vec<u32>>,
}
