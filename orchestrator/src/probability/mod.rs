//! # Probability module
//!
//! Assigns a mathematical probability curve to each planet. Each tick the curve
//! is evaluated to decide whether a sunray or asteroid is sent to that planet.
//!
//! A global `hostility` value in `[0.0, 1.0]` climbs steadily over time and is
//! blended into every planet's curve (see [`curves::ProbabilityCurve::evaluate`]) —
//! this is the "growing asteroid odds, shrinking sunray odds" ramp. Manual
//! sunray/asteroid overrides also nudge it directly.
//!
//! On every planet death:
//! - `hostility` resets to `0.0`, giving survivors a calm window before it climbs again.
//! - Surviving planets are re-assigned curves from a fresh shuffle.
//! - The phase clock keeps running (deliberately not reset — see `on_planet_death`).

pub mod bipolar;
pub mod curves;

pub use bipolar::BipolarMode;
pub use curves::{CurveKind, ProbabilityCurve};

use common_game::utils::ID;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;

/// Rate at which `hostility` climbs, tuned so a full 0.0 -> 1.0 ramp takes
/// about 60 seconds of wall-clock time.
const HOSTILITY_PER_SEC: f64 = 1.0 / 60.0;

/// How much a single manual sunray/asteroid override shifts `hostility`.
pub const MANUAL_OVERRIDE_NUDGE: f64 = 0.075;

/// Number of ticks, at game start only, where every planet is forced to
/// receive a sunray regardless of its curve or hostility. A planet that has
/// never received a sunray has zero charged energy cells and therefore
/// cannot build a rocket — its very first asteroid, however unlikely, would
/// be an undefendable instant kill. With 7 planets all rolling on the same
/// first tick, even calm ~20-30% asteroid odds each make it near-certain
/// *someone* rolls badly (roughly `1 - 0.75^7 ≈ 87%`).
///
/// One grace tick is not always enough: each of the 7 planet factories is a
/// different group's own crate with its own opaque resource pipeline (e.g.
/// Orbitron generates Hydrogen/Oxygen and combines them into Water before
/// anything rocket-related happens) — we can't know from here how many
/// charged cells a given implementation needs before `handle_asteroid` can
/// ever return `Some(rocket)`. 5 ticks (20s) is a pragmatic buffer, not a
/// hard guarantee for every possible recipe. Deliberately not re-granted on
/// later deaths/reshuffles — only brand-new planets at t=0 have zero
/// resources; survivors already have some accumulated by the time they're
/// reassigned a curve.
const GRACE_TICKS: u32 = 5;

/// Events recorded by tick.rs and drained each snapshot build.
pub enum OrchestratorEvent {
    SunraySent        { planet_id: ID },
    SunrayReceived    { planet_id: ID },
    AsteroidSent      { planet_id: ID },
    AsteroidDeflected { planet_id: ID },
    PlanetDestroyed   { planet_id: ID },
}

/// Manages probability curves for all live planets.
pub struct ProbabilityRegistry {
    /// Maps planet id to its current curve kind.
    assignments: HashMap<ID, CurveKind>,
    /// Global hostility in `[0.0, 1.0]`; climbs over time, resets on death.
    hostility: f64,
    /// Seconds elapsed since the current phase began. Resets on every flip.
    phase_elapsed: f64,
    /// Total ticks elapsed since the registry was created. Only used to gate
    /// [`GRACE_TICKS`] at game start; never reset.
    ticks_elapsed: u32,
    /// Accumulates events from the logic thread; drained by snapshot::build.
    event_buffer: Vec<OrchestratorEvent>,
}

impl ProbabilityRegistry {
    /// Creates a registry with one randomly assigned curve per planet.
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

    /// Called by tick.rs to record a game event. Vivi: call this at each send/ack site.
    pub fn record_event(&mut self, event: OrchestratorEvent) {
        self.event_buffer.push(event);
    }

    /// Drains all accumulated events. Called once per snapshot build.
    pub fn drain_events(&mut self) -> Vec<OrchestratorEvent> {
        std::mem::take(&mut self.event_buffer)
    }

    /// Advances the phase clock, the hostility ramp, and the tick counter.
    /// Call once per game tick.
    pub fn tick(&mut self, delta_secs: f64) {
        self.phase_elapsed += delta_secs;
        self.hostility = (self.hostility + HOSTILITY_PER_SEC * delta_secs).min(1.0);
        self.ticks_elapsed += 1;
    }

    /// Sunray probability for `planet_id` at the current phase time, blended
    /// by the current hostility. Returns `0.5` if the planet is not registered.
    /// Forced to `1.0` during [`GRACE_TICKS`] at game start.
    pub fn sunray_probability(&self, planet_id: ID) -> f64 {
        if self.ticks_elapsed <= GRACE_TICKS {
            return 1.0;
        }
        let Some(&kind) = self.assignments.get(&planet_id) else {
            return 0.5;
        };
        kind.build().evaluate(self.phase_elapsed, self.hostility)
    }

    /// Shifts hostility by `delta` (positive = more hostile), clamped to
    /// `[0.0, 1.0]`. Used by manual sunray/asteroid overrides.
    pub fn nudge_hostility(&mut self, delta: f64) {
        self.hostility = (self.hostility + delta).clamp(0.0, 1.0);
    }

    /// Called when a planet is destroyed. Resets hostility to give survivors
    /// a calm window, and re-shuffles curve assignments for variety.
    ///
    /// Deliberately does NOT reset the phase clock — only `hostility`. Each
    /// planet's own curve keeps running from wherever it was, so a death
    /// doesn't synchronize every survivor's wobble to the same instant.
    pub fn on_planet_death(&mut self, planet_id: ID, rng: &mut impl Rng) {
        self.assignments.remove(&planet_id);
        self.hostility = 0.0;

        let surviving: Vec<ID> = self.assignments.keys().copied().collect();
        self.assignments = Self::shuffle_assignments(&surviving, rng);

        log::info!(
            "Planet {planet_id} destroyed - hostility reset to 0, {} planets remain",
            self.assignments.len(),
        );
    }

    /// Derived personality label (`hostility >= 0.5` => Eclipse) for logs/GUI.
    pub fn bipolar_mode(&self) -> BipolarMode {
        BipolarMode::from_hostility(self.hostility)
    }

    pub fn hostility(&self) -> f64 {
        self.hostility
    }

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
        assert!(!all_guaranteed, "grace period should not extend past GRACE_TICKS");
    }
}
