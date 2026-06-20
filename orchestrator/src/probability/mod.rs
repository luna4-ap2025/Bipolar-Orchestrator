//! # Probability module
//!
//! Each live planet is assigned a **probability curve** that controls the
//! likelihood of a sunray vs an asteroid being sent to it on each game tick.
//!
//! ## Design
//! - Every curve maps a time `t: f64` (seconds elapsed) to a value in `[0.0, 1.0]`
//!   representing the **sunray probability** for that tick.
//! - The **asteroid probability** is simply `1.0 - sunray_prob`.
//! - When a planet dies the **bipolar counterpart** is activated: all remaining
//!   planets switch to the mathematical counterpart of their assigned curve.
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
    /// Current bipolar mode state (tracks deaths and which "side" we're on).
    bipolar: BipolarMode,
    /// Seconds elapsed since the logic loop started, advanced each tick.
    elapsed: f64,
}

impl ProbabilityRegistry {
    /// Constructs a registry and **randomly assigns** one curve per planet.
    ///
    /// Each planet gets a unique curve kind where possible (falls back to
    /// repeating when there are more planets than curve variants).
    pub fn new(planet_ids: impl Iterator<Item = ID>, rng: &mut impl Rng) -> Self {
        let ids: Vec<ID> = planet_ids.collect();
        let mut all_kinds: Vec<CurveKind> = CurveKind::all();
        all_kinds.shuffle(rng);

        let assignments = ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, all_kinds[i % all_kinds.len()]))
            .collect();

        Self {
            assignments,
            bipolar: BipolarMode::Normal,
            elapsed: 0.0,
        }
    }

    /// Advances the internal clock by `delta_secs` (call once per tick).
    pub fn tick(&mut self, delta_secs: f64) {
        self.elapsed += delta_secs;
    }

    /// Returns the sunray probability for `planet_id` at the current time.
    ///
    /// Returns `0.5` if the planet is not registered (should not happen in
    /// normal operation).
    pub fn sunray_probability(&self, planet_id: ID) -> f64 {
        let Some(&kind) = self.assignments.get(&planet_id) else {
            return 0.5;
        };

        let curve = kind.build(self.bipolar);
        curve.evaluate(self.elapsed)
    }

    /// Notifies the registry that a planet has been destroyed.
    ///
    /// Removes the planet's assignment and triggers the bipolar counter-phase
    /// if this is the first death (or every subsequent death).
    pub fn on_planet_death(&mut self, planet_id: ID) {
        self.assignments.remove(&planet_id);
        self.bipolar = self.bipolar.next();
        log::info!(
            "Planet {planet_id} destroyed - bipolar mode is now {:?}",
            self.bipolar
        );
    }

    /// Returns the current [`BipolarMode`].
    pub fn bipolar_mode(&self) -> BipolarMode {
        self.bipolar
    }
}
