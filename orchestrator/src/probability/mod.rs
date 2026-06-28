//! # Probability module
//!
//! Assigns a mathematical probability curve to each planet. Each tick the curve
//! is evaluated to decide whether a sunray or asteroid is sent to that planet.
//!
//! On every planet death:
//! - The active personality flips (SOLACE <-> ECLIPSE).
//! - Surviving planets are re-assigned curves from a fresh shuffle.
//! - The phase clock resets to zero so all curves restart from their opening value.

pub mod bipolar;
pub mod curves;

pub use bipolar::BipolarMode;
pub use curves::{CurveKind, ProbabilityCurve};

use common_game::utils::ID;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;

/// Manages probability curves for all live planets.
pub struct ProbabilityRegistry {
    /// Maps planet id to its current curve kind.
    assignments: HashMap<ID, CurveKind>,
    /// Currently active personality.
    bipolar: BipolarMode,
    /// Seconds elapsed since the current phase began. Resets on every flip.
    phase_elapsed: f64,
}

impl ProbabilityRegistry {
    /// Creates a registry with one randomly assigned curve per planet.
    pub fn new(planet_ids: impl Iterator<Item = ID>, rng: &mut impl Rng) -> Self {
        let ids: Vec<ID> = planet_ids.collect();
        let assignments = Self::shuffle_assignments(&ids, rng);
        Self {
            assignments,
            bipolar: BipolarMode::Solace,
            phase_elapsed: 0.0,
        }
    }

    /// Advances the phase clock. Call once per game tick.
    pub fn tick(&mut self, delta_secs: f64) {
        self.phase_elapsed += delta_secs;
    }

    /// Sunray probability for `planet_id` at the current phase time.
    /// Returns `0.5` if the planet is not registered.
    pub fn sunray_probability(&self, planet_id: ID) -> f64 {
        let Some(&kind) = self.assignments.get(&planet_id) else {
            return 0.5;
        };
        kind.build(self.bipolar).evaluate(self.phase_elapsed)
    }

    /// Called when a planet is destroyed. Flips the personality, re-shuffles
    /// curve assignments for surviving planets, and resets the phase clock.
    pub fn on_planet_death(&mut self, planet_id: ID, rng: &mut impl Rng) {
        self.assignments.remove(&planet_id);
        self.bipolar = self.bipolar.next();
        self.phase_elapsed = 0.0;

        let surviving: Vec<ID> = self.assignments.keys().copied().collect();
        self.assignments = Self::shuffle_assignments(&surviving, rng);

        log::info!(
            "Planet {planet_id} destroyed - {} now active, {} planets remain",
            if self.bipolar.is_eclipse() { "ECLIPSE" } else { "SOLACE" },
            self.assignments.len(),
        );
    }

    pub fn bipolar_mode(&self) -> BipolarMode {
        self.bipolar
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
