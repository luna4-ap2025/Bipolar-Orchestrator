//! # Probability module
//!
//! Each live planet is assigned a **probability curve** that controls the
//! likelihood of a sunray vs an asteroid being sent to it on each game tick.
//!
//! ## Design
//! - Every curve maps a time `t: f64` (seconds elapsed *in the current phase*)
//!   to a value in `[0.0, 1.0]` representing the **sunray probability** for that tick.
//! - The **asteroid probability** is simply `1.0 - sunray_prob`.
//! - When a planet dies the active personality flips (SOLACE ↔ ECLIPSE), surviving
//!   planets are **re-shuffled** with fresh curve assignments, and the phase clock resets.
//!
//! ## Owner: Vale (curves + registry) + Vivi (bipolar integration into logic loop)

pub mod bipolar;
pub mod curves;

pub use bipolar::BipolarMode;
pub use curves::{CurveKind, ProbabilityCurve};

use common_game::utils::ID;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;

/// Manages probability curves for all alive planets.
///
/// Constructed once at game start and updated when planets die.
pub struct ProbabilityRegistry {
    /// Maps planet id → its currently active curve kind.
    assignments: HashMap<ID, CurveKind>,
    /// Current active personality (SOLACE or ECLIPSE).
    bipolar: BipolarMode,
    /// Seconds elapsed since the **current phase began** (resets on every flip).
    /// Used by all curves so they always start from t=0 in each new phase.
    phase_elapsed: f64,
}

impl ProbabilityRegistry {
    /// Constructs a registry and **randomly assigns** one curve per planet.
    ///
    /// Each planet gets a unique curve kind where possible (falls back to
    /// repeating when there are more planets than curve variants).
    pub fn new(planet_ids: impl Iterator<Item = ID>, rng: &mut impl Rng) -> Self {
        let ids: Vec<ID> = planet_ids.collect();
        let assignments = Self::shuffle_assignments(&ids, rng);

        Self {
            assignments,
            bipolar: BipolarMode::Solace,
            phase_elapsed: 0.0,
        }
    }

    /// Advances the phase clock by `delta_secs` (call once per tick).
    pub fn tick(&mut self, delta_secs: f64) {
        self.phase_elapsed += delta_secs;
    }

    /// Returns the sunray probability for `planet_id` at the current phase time.
    ///
    /// Returns `0.5` if the planet is not registered (should not happen in
    /// normal operation).
    pub fn sunray_probability(&self, planet_id: ID) -> f64 {
        let Some(&kind) = self.assignments.get(&planet_id) else {
            return 0.5;
        };

        let curve = kind.build(self.bipolar);
        curve.evaluate(self.phase_elapsed)
    }

    /// Notifies the registry that a planet has been destroyed.
    ///
    /// - Removes the dead planet.
    /// - Flips the active personality (SOLACE ↔ ECLIPSE).
    /// - Re-shuffles curve assignments across surviving planets.
    /// - Resets the phase clock to 0.
    pub fn on_planet_death(&mut self, planet_id: ID, rng: &mut impl Rng) {
        self.assignments.remove(&planet_id);

        self.bipolar = self.bipolar.next();
        self.phase_elapsed = 0.0;

        // Re-shuffle curves across surviving planets so each phase feels fresh.
        let surviving: Vec<ID> = self.assignments.keys().copied().collect();
        self.assignments = Self::shuffle_assignments(&surviving, rng);

        log::info!(
            "Planet {planet_id} destroyed — {} takes control. {} planets remain.",
            if self.bipolar.is_eclipse() { "ECLIPSE" } else { "SOLACE" },
            self.assignments.len(),
        );
    }

    /// Returns the current active personality.
    pub fn bipolar_mode(&self) -> BipolarMode {
        self.bipolar
    }

    /// Returns seconds elapsed in the current phase.
    pub fn phase_elapsed(&self) -> f64 {
        self.phase_elapsed
    }

    /// Randomly assigns one curve kind per planet id.
    fn shuffle_assignments(ids: &[ID], rng: &mut impl Rng) -> HashMap<ID, CurveKind> {
        let mut all_kinds = CurveKind::all();
        all_kinds.shuffle(rng);
        ids.iter()
            .enumerate()
            .map(|(i, &id)| (id, all_kinds[i % all_kinds.len()]))
            .collect()
    }
}
