//! # Probability curves
//!
//! Each variant of [`CurveKind`] represents a distinct mathematical function
//! evaluated at `t = phase_elapsed` (seconds since the current phase began).
//!
//! All curves return a value in `[0.0, 1.0]` representing sunray probability.
//! ECLIPSE inverts every curve via `1.0 - value`.
//!
//! ## Curve behaviour at phase start (t = 0)
//!
//! | Kind          | SOLACE starts at | ECLIPSE starts at | Character                        |
//! |---------------|------------------|-------------------|----------------------------------|
//! | `Sine`        | 0.5 (neutral)    | 0.5 (neutral)     | Smooth oscillation, no extremes  |
//! | `Cosine`      | 1.0 (full sun)   | 0.0 (full asteroid)| Cleanest narrative arc          |
//! | `Sawtooth`    | 1.0 (full sun)   | 0.0 (full asteroid)| Linear slide, then hard reset   |
//! | `Triangle`    | 1.0 (full sun)   | 0.0 (full asteroid)| Symmetric rise and fall         |
//! | `Square`      | 1.0 (full sun)   | 0.0 (full asteroid)| Hard snaps, no middle ground    |
//! | `Exponential` | 1.0 (full sun)   | 0.0 (full asteroid)| Starts decisive, fades to chaos |
//!
//! ## Owner: Vale

use super::BipolarMode;

/// Period in seconds for oscillating curves.
const PERIOD: f64 = 10.0;
/// Decay constant for the exponential curve.
const DECAY_K: f64 = 0.05;

/// Identifies a curve family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveKind {
    Sine,
    Cosine,
    Sawtooth,
    Triangle,
    Square,
    Exponential,
}

impl CurveKind {
    /// Returns all available curve kinds.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Sine,
            Self::Cosine,
            Self::Sawtooth,
            Self::Triangle,
            Self::Square,
            Self::Exponential,
        ]
    }

    /// Constructs the concrete [`ProbabilityCurve`] for this kind under the given personality.
    pub fn build(self, mode: BipolarMode) -> ProbabilityCurve {
        ProbabilityCurve { kind: self, mode }
    }
}

/// A concrete, evaluable probability curve.
///
/// Constructed via [`CurveKind::build`].
pub struct ProbabilityCurve {
    pub(super) kind: CurveKind,
    pub(super) mode: BipolarMode,
}

impl ProbabilityCurve {
    /// Evaluates the curve at `t` seconds into the current phase.
    ///
    /// Returns a value in `[0.0, 1.0]` representing sunray probability.
    /// ECLIPSE inverts the result so high values become low and vice versa.
    #[must_use]
    pub fn evaluate(&self, t: f64) -> f64 {
        let solace_value = self.evaluate_solace(t);
        match self.mode {
            BipolarMode::Solace => solace_value,
            BipolarMode::Eclipse => 1.0 - solace_value,
        }
    }

    /// Evaluates the curve as SOLACE would use it — the "base" form.
    /// All curves start at or near 1.0 so SOLACE opens each phase nurturing.
    fn evaluate_solace(&self, t: f64) -> f64 {
        match self.kind {
            // Smooth oscillation between 0 and 1. Starts at 0.5 (neutral).
            // SOLACE: gently rises then falls. Neither extreme at start.
            CurveKind::Sine => (t.sin() + 1.0) / 2.0,

            // Smooth oscillation, starts at 1.0 (full sunray).
            // SOLACE opens fully in control and gracefully loses grip.
            // Best narrative curve — mirror-perfect for both sides.
            CurveKind::Cosine => (t.cos() + 1.0) / 2.0,

            // Linear slide from 1.0 → 0.0 over one period, then hard reset.
            // SOLACE: starts nurturing, slowly bleeds toward destruction, snaps back.
            // Fixed from old version (was 0→1, wrong direction for SOLACE).
            CurveKind::Sawtooth => {
                let phase = t % PERIOD;
                1.0 - phase / PERIOD
            }

            // Symmetric: falls 1→0 in first half, rises 0→1 in second half.
            // SOLACE: opens with grace, hits a destructive valley, recovers.
            // Fixed from old version (was rising first, starting at 0).
            CurveKind::Triangle => {
                let phase = t % PERIOD;
                let half = PERIOD / 2.0;
                if phase < half {
                    1.0 - phase / half        // falls from 1.0 → 0.0
                } else {
                    (phase - half) / half     // rises from 0.0 → 1.0
                }
            }

            // Hard snap: 1.0 for first half-period, 0.0 for second half.
            // SOLACE: fully in control, then ECLIPSE violently seizes it.
            // No gradual bleed — abrupt personality takeover mid-phase.
            CurveKind::Square => {
                let phase = t % PERIOD;
                if phase < PERIOD / 2.0 { 1.0 } else { 0.0 }
            }

            // Starts at 1.0 and decays toward 0. Uses phase_elapsed (not
            // global time) so it always starts fresh at 1.0 each new phase.
            // SOLACE: opens fully nurturing but loses grip permanently as
            // the phase ages. The longer she holds on, the more chaos grows.
            CurveKind::Exponential => (-DECAY_K * t).exp(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solace(kind: CurveKind) -> ProbabilityCurve {
        kind.build(BipolarMode::Solace)
    }

    fn eclipse(kind: CurveKind) -> ProbabilityCurve {
        kind.build(BipolarMode::Eclipse)
    }

    #[test]
    fn all_curves_return_values_in_unit_interval() {
        for kind in CurveKind::all() {
            for t in [0.0_f64, 1.0, 5.0, 10.0, 100.0] {
                let v = solace(kind).evaluate(t);
                assert!(
                    (0.0..=1.0).contains(&v),
                    "{kind:?} at t={t} returned {v} outside [0,1]"
                );
            }
        }
    }

    #[test]
    fn eclipse_inverts_solace() {
        for kind in CurveKind::all() {
            let t = 3.7_f64;
            let s = solace(kind).evaluate(t);
            let e = eclipse(kind).evaluate(t);
            assert!(
                (s + e - 1.0).abs() < 1e-10,
                "{kind:?}: solace={s} + eclipse={e} should sum to 1"
            );
        }
    }

    #[test]
    fn solace_curves_start_high_or_neutral() {
        // All SOLACE curves should start at >= 0.5 (leaning nurturing)
        for kind in CurveKind::all() {
            let v = solace(kind).evaluate(0.0);
            assert!(
                v >= 0.5,
                "{kind:?} SOLACE starts at {v}, expected >= 0.5"
            );
        }
    }

    #[test]
    fn eclipse_curves_start_low_or_neutral() {
        // All ECLIPSE curves should start at <= 0.5 (leaning destructive)
        for kind in CurveKind::all() {
            let v = eclipse(kind).evaluate(0.0);
            assert!(
                v <= 0.5,
                "{kind:?} ECLIPSE starts at {v}, expected <= 0.5"
            );
        }
    }

    #[test]
    fn exponential_starts_at_one_and_decays() {
        let c = solace(CurveKind::Exponential);
        let start = c.evaluate(0.0);
        let later = c.evaluate(100.0);
        assert!((start - 1.0).abs() < 1e-10, "Starts at 1.0");
        assert!(start > later, "Decays over time");
    }

    #[test]
    fn sawtooth_starts_at_one_and_falls() {
        let c = solace(CurveKind::Sawtooth);
        assert!((c.evaluate(0.0) - 1.0).abs() < 1e-10, "Starts at 1.0");
        assert!(c.evaluate(5.0) < c.evaluate(0.0), "Falls over time");
    }

    #[test]
    fn triangle_starts_at_one_and_falls_first() {
        let c = solace(CurveKind::Triangle);
        assert!((c.evaluate(0.0) - 1.0).abs() < 1e-10, "Starts at 1.0");
        assert!(c.evaluate(2.5) < c.evaluate(0.0), "Falls in first half");
    }
}
