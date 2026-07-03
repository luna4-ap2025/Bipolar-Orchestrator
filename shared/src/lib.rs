/// Which orchestrator personality is currently in control.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Personality {
    #[default]
    Solace,
    Eclipse,
}

/// Mirrors `common_game::components::resource::ResourceType` without pulling
/// that crate into this dependency-free shared crate. The orchestrator maps
/// the real type onto this one in `snapshot::build`.
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

/// One explorer's current position and real carried-resource bag content
/// (fetched live from the explorer via `OrchestratorApi::bag_content`, not
/// guessed from GUI-side state).
#[derive(Clone, Debug)]
pub struct ExplorerSnapshot {
    pub id: u32,
    pub planet: u32,
    pub bag: Vec<(ResourceKind, usize)>,
}

/// A discrete event that fired during one logic tick.
#[derive(Clone, Debug)]
pub enum GalaxyEvent {
    SunraySent        { planet_id: u32 },
    SunrayReceived    { planet_id: u32 },
    AsteroidSent      { planet_id: u32 },
    AsteroidDeflected { planet_id: u32 },
    PlanetDestroyed   { planet_id: u32 },
    ExplorerKilled    { explorer_id: u32 },
    ExplorerMoved     { explorer_id: u32, from: u32, to: u32 },
}

/// Full state of the galaxy, updated ~4 times per second by the bridge thread.
#[derive(Clone, Debug, Default)]
pub struct GalaxySnapshot {
    pub personality: Personality,
    /// Global hostility in `[0.0, 1.0]`; drives the Solace/Eclipse crossfade.
    pub hostility: f64,
    pub phase_elapsed: f64,
    pub alive_planets: Vec<u32>,
    pub explorers: Vec<ExplorerSnapshot>,
    /// Events that fired since the last snapshot. Drained on each poll.
    pub events: Vec<GalaxyEvent>,
    /// Current live adjacency: planet id -> its currently-alive neighbor ids.
    /// The real topology, not a hardcoded ring — shrinks as planets die.
    pub neighbors: std::collections::HashMap<u32, Vec<u32>>,
}
