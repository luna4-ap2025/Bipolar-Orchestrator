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

/// Rate at which `hostility` climbs. The 480s (8-minute) ramp assumed a
/// ~10-minute target full-game length, but real playtests (this project's
/// own logs) show games actually ending via total galaxy wipeout in about
/// 2-4 minutes — meaning a natural, un-forced climb to the 0.5 Eclipse
/// threshold (`HOSTILITY_PER_SEC * elapsed_secs`, minus death relief) almost
/// never had enough real time to get there before the game was already over.
/// That's the actual mechanism behind "Eclipse never triggers on her own" —
/// not a bug, a pacing mismatch between the ramp and the real game length.
/// Reverted to 240s (4 minutes), the historical middle value, so a full
/// climb fits inside an actual game's real lifespan. History: 1/60 (60s) ->
/// 1/240 (4 min) -> 1/480 (8 min) -> 1/240 (4 min, this fix).
const HOSTILITY_PER_SEC: f64 = 1.0 / 240.0;

/// How much a single manual sunray/asteroid override shifts `hostility`.
pub const MANUAL_OVERRIDE_NUDGE: f64 = 0.075;

/// Flat hostility relief granted on every planet death (see
/// `on_planet_death` for why this replaced a multiplicative retention).
/// Set just under `HOSTILITY_PER_SEC * 25.0` (one `TICK_INTERVAL`'s worth of
/// climb), so even the fastest possible death cadence still nets a small
/// positive drift toward the Eclipse threshold over time.
const HOSTILITY_DEATH_RELIEF: f64 = 0.04;

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
/// ever return `Some(rocket)`.
///
/// Was 5 ticks — set when `TICK_INTERVAL` was 4s (20s total). Never revisited
/// after `TICK_INTERVAL` was raised to 25s for pacing, so grace silently
/// became 125s (over 2 minutes) of forced safety before anything could
/// happen — felt like being stuck "recharging" at the start. 2 ticks (50s)
/// is still more real time than the original 20s design, while cutting the
/// dead time by more than half. Deliberately not re-granted on
/// later deaths/reshuffles — only brand-new planets at t=0 have zero
/// resources; survivors already have some accumulated by the time they're
/// reassigned a curve.
const GRACE_TICKS: u32 = 2;

/// Events recorded by tick.rs and drained each snapshot build.
#[derive(Debug)]
pub enum OrchestratorEvent {
    SunraySent        { planet_id: ID },
    SunrayReceived    { planet_id: ID },
    AsteroidSent      { planet_id: ID },
    AsteroidDeflected { planet_id: ID },
    PlanetDestroyed   { planet_id: ID },
    ExplorerKilled    { explorer_id: ID },
    ExplorerMoved     { explorer_id: ID, from: ID, to: ID },
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

    /// Called when a planet is destroyed. Dampens hostility (relief, not a
    /// full wipe) and re-shuffles curve assignments for variety.
    ///
    /// Was a multiplicative 65% retention (`hostility *= 0.65`), tuned against
    /// an assumed ~90-100s gap between deaths. Real playtests instead showed
    /// deaths landing every 10-75s once the galaxy started dying off (the
    /// one-death-per-tick cap still allows one every `TICK_INTERVAL` = 25s).
    /// At that real cadence the multiplicative retention compounds hard —
    /// three deaths 25s apart leave only `0.65^3 ≈ 27%` of any accumulated
    /// hostility — so it was permanently crushing hostility back toward zero
    /// and Eclipse effectively never triggered, which reads as Solace
    /// dominating the whole game against her own character (she doesn't want
    /// to be the one in control that long).
    ///
    /// Switched to a flat subtraction instead: `HOSTILITY_DEATH_RELIEF` is
    /// chosen to be just under one tick's worth of climb
    /// (`HOSTILITY_PER_SEC * TICK_INTERVAL ≈ 0.052`), so even worst-case
    /// back-to-back-tick deaths leave a small net *positive* drift in
    /// hostility instead of a guaranteed crush — hostility can still ratchet
    /// up to the Eclipse threshold over a long enough game, while every death
    /// still gives real, immediate relief.
    ///
    /// Deliberately does NOT reset the phase clock — only `hostility`. Each
    /// planet's own curve keeps running from wherever it was, so a death
    /// doesn't synchronize every survivor's wobble to the same instant.
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
