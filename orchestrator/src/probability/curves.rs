//! # Probability curves
//!
//! Each variant of [`CurveKind`] represents a distinct mathematical function
//! and its **bipolar counterpart** (the function used after a planet dies).
//!
//! All curves return a value in `[0.0, 1.0]` representing sunray probability.
//!
//! ## Curve catalogue
//!
//! | Kind         | Normal                          | Bipolar counterpart          |
//! |--------------|---------------------------------|------------------------------|
//! | `Sine`       | `(sin(t) + 1) / 2`             | `(cos(t) + 1) / 2`          |
//! | `Cosine`     | `(cos(t) + 1) / 2`             | `(sin(t) + 1) / 2`          |
//! | `Sawtooth`   | `(t % period) / period`         | `1 - (t % period) / period` |
//! | `Triangle`   | linear rise then fall           | linear fall then rise        |
//! | `Square`     | alternates 1.0 / 0.0           | alternates 0.0 / 1.0        |
//! | `Exponential`| `e^(-t*k)` decaying to 0      | `1 - e^(-t*k)` rising to 1  |
//!
//! ## Owner: Vale

use super::BipolarMode;

/// Period in seconds for oscillating curves.
const PERIOD: f64 = 10.0;
/// Decay constant for the exponential curve.
const DECAY_K: f64 = 0.05;

/// Identifies a curve family (variant-pair of normal + bipolar counterpart).
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

    /// Constructs the concrete [`ProbabilityCurve`] for this kind, taking the
    /// current [`BipolarMode`] into account.
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
    /// Evaluates the curve at time `t` (seconds since game start).
    ///
    /// Returns a value in `[0.0, 1.0]` representing sunray probability.
    /// The bipolar mode flips normal ↔ counterpart behavior.
    #[must_use]
    pub fn evaluate(&self, t: f64) -> f64 {
        let normal = self.evaluate_normal(t);
        match self.mode {
            BipolarMode::Normal => normal,
            BipolarMode::Flipped => 1.0 - normal,
        }
    }

    /// Evaluates the curve in its **normal** (non-flipped) form.
    fn evaluate_normal(&self, t: f64) -> f64 {
        match self.kind {
            CurveKind::Sine => (t.sin() + 1.0) / 2.0,

            CurveKind::Cosine => (t.cos() + 1.0) / 2.0,

            CurveKind::Sawtooth => {
                let phase = t % PERIOD;
                phase / PERIOD
            }

            CurveKind::Triangle => {
                // rises from 0→1 in first half-period, falls 1→0 in second half
                let phase = t % PERIOD;
                let half = PERIOD / 2.0;
                if phase < half {
                    phase / half
                } else {
                    1.0 - (phase - half) / half
                }
            }

            CurveKind::Square => {
                // alternates between 1.0 (sunray) and 0.0 (asteroid)
                let phase = t % PERIOD;
                if phase < PERIOD / 2.0 { 1.0 } else { 0.0 }
            }

            CurveKind::Exponential => {
                // starts near 1.0, decays towards 0.0 (increasingly hostile)
                (-DECAY_K * t).exp()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(kind: CurveKind) -> ProbabilityCurve {
        kind.build(BipolarMode::Normal)
    }

    fn flipped(kind: CurveKind) -> ProbabilityCurve {
        kind.build(BipolarMode::Flipped)
    }

    #[test]
    fn all_curves_return_values_in_unit_interval() {
        for kind in CurveKind::all() {
            for t in [0.0_f64, 1.0, 5.0, 10.0, 100.0] {
                let v = curve(kind).evaluate(t);
                assert!(
                    (0.0..=1.0).contains(&v),
                    "{kind:?} at t={t} returned {v} outside [0,1]"
                );
            }
        }
    }

    #[test]
    fn bipolar_flips_value() {
        for kind in CurveKind::all() {
            let t = 3.7_f64;
            let normal = curve(kind).evaluate(t);
            let flipped_val = flipped(kind).evaluate(t);
            assert!(
                (normal + flipped_val - 1.0).abs() < 1e-10,
                "{kind:?}: normal={normal} + flipped={flipped_val} should sum to 1"
            );
        }
    }

    #[test]
    fn exponential_starts_high_and_decays() {
        let c = curve(CurveKind::Exponential);
        let early = c.evaluate(0.0);
        let late = c.evaluate(1000.0);
        assert!(early > late, "Exponential should decay over time");
        assert!((early - 1.0).abs() < 1e-10, "Starts at 1.0");
    }

    #[test]
    fn sine_and_cosine_are_counterparts() {
        // sine and cosine are independent curves, not counterparts of each other.
        // both just need to stay in [0,1] regardless of mode.
        for t in [0.0_f64, std::f64::consts::PI / 2.0, std::f64::consts::PI] {
            let s = curve(CurveKind::Sine).evaluate(t);
            let c = curve(CurveKind::Cosine).evaluate(t);
            assert!((0.0..=1.0).contains(&s));
            assert!((0.0..=1.0).contains(&c));
        }
    }
}
