//! Each planet gets a probability curve, evaluated every tick to decide if it
//! gets a sunray or an asteroid.
//!
//! `hostility` (0.0 - 1.0) goes up over time and is mixed into every curve, so
//! asteroids get more likely as the game goes on. Manual sunrays/asteroids
//! also move it a bit. When a planet dies, hostility goes down a little and
//! the surviving planets get new curves.

pub mod bipolar;
pub mod curves;

pub use bipolar::BipolarMode;
pub use curves::{CurveKind, ProbabilityCurve};

use common_game::utils::ID;
use rand::Rng;
use rand::seq::SliceRandom;
use std::collections::HashMap;

// 4 minutes to go from 0 to 1. Games usually end in 2-4 minutes, and with a
// slower ramp Eclipse almost never got to take control.
const HOSTILITY_PER_SEC: f64 = 1.0 / 240.0;

/// How much one manual sunray/asteroid moves `hostility`.
pub const MANUAL_OVERRIDE_NUDGE: f64 = 0.075;

// A bit less than what hostility gains in one tick (25s * 1/240 = ~0.052), so
// even if planets die every tick hostility still slowly goes up.
const HOSTILITY_DEATH_RELIEF: f64 = 0.04;

// At the start every planet gets only sunrays for a couple of ticks. A new
// planet has no charged cells, so it can't build a rocket, and with 7 planets
// rolling at once one of them almost always got an asteroid on the first
// tick and died instantly. Some planets need more than one charge before they
// can build a rocket, so one tick wasn't always enough.
// Only at the start, survivors of later deaths already have charge.
const GRACE_TICKS: u32 = 2;

/// Things that happened during a tick, for the GUI.
#[derive(Debug)]
pub enum OrchestratorEvent {
    SunraySent { planet_id: ID },
    SunrayReceived { planet_id: ID },
    AsteroidSent { planet_id: ID },
    AsteroidDeflected { planet_id: ID },
    PlanetDestroyed { planet_id: ID },
    ExplorerKilled { explorer_id: ID },
    ExplorerMoved { explorer_id: ID, from: ID, to: ID },
}

pub struct ProbabilityRegistry {
    assignments: HashMap<ID, CurveKind>,
    hostility: f64,
    // seconds since the game started, the curves are evaluated at this time
    phase_elapsed: f64,
    // only used for GRACE_TICKS
    ticks_elapsed: u32,
    // filled by the logic thread, emptied by snapshot::build
    event_buffer: Vec<OrchestratorEvent>,
}

impl ProbabilityRegistry {
    /// Gives each planet a random curve.
    pub fn new(planet_ids: impl Iterator<Item = ID>, rng: &mut impl Rng) -> Self {
        let ids: Vec<ID> = planet_ids.collect();
        let assignments = Self::shuffle_assignments(&ids, rng);
        Self {
            assignments,
            hostility: 0.0,
            phase_elapsed: 0.0,
            ticks_elapsed: 0,
            event_buffer: Vec::new(),
        }
    }

    pub fn record_event(&mut self, event: OrchestratorEvent) {
        self.event_buffer.push(event);
    }

    pub fn drain_events(&mut self) -> Vec<OrchestratorEvent> {
        std::mem::take(&mut self.event_buffer)
    }

    /// Call once per game tick.
    pub fn tick(&mut self, delta_secs: f64) {
        self.phase_elapsed += delta_secs;
        self.hostility = (self.hostility + HOSTILITY_PER_SEC * delta_secs).min(1.0);
        self.ticks_elapsed += 1;
    }

    /// Chance that `planet_id` gets a sunray this tick (otherwise an asteroid).
    /// Always 1.0 during the grace ticks, 0.5 for unknown planets.
    #[must_use]
    pub fn sunray_probability(&self, planet_id: ID) -> f64 {
        if self.ticks_elapsed <= GRACE_TICKS {
            return 1.0;
        }
        let Some(&kind) = self.assignments.get(&planet_id) else {
            return 0.5;
        };
        kind.build().evaluate(self.phase_elapsed, self.hostility)
    }

    /// Positive = more hostile. Used by manual sunrays/asteroids.
    pub fn nudge_hostility(&mut self, delta: f64) {
        self.hostility = (self.hostility + delta).clamp(0.0, 1.0);
    }

    // Hostility goes down by a fixed amount. Before it was multiplied by 0.65,
    // but planets die every ~25s near the end and it kept getting crushed back
    // to 0, so Eclipse never showed up.
    // The phase clock is not reset, otherwise all the curves would line up.
    pub fn on_planet_death(&mut self, planet_id: ID, rng: &mut impl Rng) {
        self.assignments.remove(&planet_id);
        self.hostility = (self.hostility - HOSTILITY_DEATH_RELIEF).max(0.0);

        let surviving: Vec<ID> = self.assignments.keys().copied().collect();
        self.assignments = Self::shuffle_assignments(&surviving, rng);

        log::info!(
            "Planet {planet_id} destroyed - hostility dampened to {:.3}, {} planets remain",
            self.hostility,
            self.assignments.len(),
        );
    }

    /// Eclipse when hostility >= 0.5, Solace otherwise.
    #[must_use]
    pub fn bipolar_mode(&self) -> BipolarMode {
        BipolarMode::from_hostility(self.hostility)
    }

    #[must_use]
    pub fn hostility(&self) -> f64 {
        self.hostility
    }

    #[must_use]
    pub fn phase_elapsed(&self) -> f64 {
        self.phase_elapsed
    }

    fn shuffle_assignments(ids: &[ID], rng: &mut impl Rng) -> HashMap<ID, CurveKind> {
        let mut kinds = CurveKind::all();
        kinds.shuffle(rng);
        ids.iter()
            .enumerate()
            .map(|(i, &id)| (id, kinds[i % kinds.len()]))
            .collect()
    }
}

#[cfg(test)]
// comparing with == is fine here, grace ticks return exactly 1.0
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn grace_ticks_guarantee_sunray_for_every_planet() {
        let mut rng = rand::rng();
        let mut reg = ProbabilityRegistry::new(1..=7, &mut rng);

        for _ in 0..GRACE_TICKS {
            reg.tick(4.0);
            for id in 1..=7 {
                assert_eq!(
                    reg.sunray_probability(id),
                    1.0,
                    "planet {id} should be guaranteed a sunray during the grace period"
                );
            }
        }
    }

    #[test]
    fn grace_period_ends_after_grace_ticks() {
        let mut rng = rand::rng();
        let mut reg = ProbabilityRegistry::new(1..=7, &mut rng);

        for _ in 0..=GRACE_TICKS {
            reg.tick(4.0);
        }

        let all_guaranteed = (1..=7).all(|id| reg.sunray_probability(id) == 1.0);
        assert!(
            !all_guaranteed,
            "grace period should not extend past GRACE_TICKS"
        );
    }
}
