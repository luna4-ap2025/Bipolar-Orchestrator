/// Which orchestrator personality is currently in control.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Personality {
    #[default]
    Solace,
    Eclipse,
}

/// One explorer's current position.
#[derive(Clone, Debug)]
pub struct ExplorerSnapshot {
    pub id: u32,
    pub planet: u32,
}

/// A discrete event that fired during one logic tick.
#[derive(Clone, Debug)]
pub enum GalaxyEvent {
    SunraySent        { planet_id: u32 },
    SunrayReceived    { planet_id: u32 },
    AsteroidSent      { planet_id: u32 },
    AsteroidDeflected { planet_id: u32 },
    PlanetDestroyed   { planet_id: u32 },
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
}
