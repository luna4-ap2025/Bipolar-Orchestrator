//! # Probability curves
//!
//! Each [`CurveKind`] maps phase time `t` (seconds since last personality flip)
//! to a sunray probability in `[0.0, 1.0]`. ECLIPSE inverts each curve via
//! `1.0 - value`, so a curve that starts at 1.0 under SOLACE starts at 0.0
//! under ECLIPSE.
//!
//! All curves are designed to open at >= 0.5 under SOLACE and <= 0.5 under
//! ECLIPSE, so each personality starts the phase leaning in its expected direction.
//!
//! | Kind          | SOLACE t=0 | ECLIPSE t=0 | Shape                            |
//! |---------------|------------|-------------|----------------------------------|
//! | `Sine`        | 0.5        | 0.5         | smooth oscillation, neutral open |
//! | `Cosine`      | 1.0        | 0.0         | smooth oscillation, full open    |
//! | `Sawtooth`    | 1.0        | 0.0         | linear fall then hard reset      |
//! | `Triangle`    | 1.0        | 0.0         | falls then rises, symmetric      |
//! | `Square`      | 1.0        | 0.0         | hard binary alternation          |
//! | `Exponential` | 1.0        | 0.0         | decays toward 0, no recovery     |

use super::BipolarMode;

const PERIOD: f64 = 10.0;
const DECAY_K: f64 = 0.05;

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

    pub fn build(self, mode: BipolarMode) -> ProbabilityCurve {
        ProbabilityCurve { kind: self, mode }
    }
}

pub struct ProbabilityCurve {
    pub(super) kind: CurveKind,
    pub(super) mode: BipolarMode,
}

impl ProbabilityCurve {
    /// Sunray probability at `t` seconds into the current phase.
    #[must_use]
    pub fn evaluate(&self, t: f64) -> f64 {
        let solace_value = self.evaluate_solace(t);
        match self.mode {
            BipolarMode::Solace => solace_value,
            BipolarMode::Eclipse => 1.0 - solace_value,
        }
    }

    fn evaluate_solace(&self, t: f64) -> f64 {
        match self.kind {
            // (sin(t) + 1) / 2 — oscillates [0, 1], starts at 0.5
            CurveKind::Sine => (t.sin() + 1.0) / 2.0,

            // (cos(t) + 1) / 2 — oscillates [0, 1], starts at 1.0
            CurveKind::Cosine => (t.cos() + 1.0) / 2.0,

            // 1 - (t mod T) / T — linear fall from 1.0 to 0.0, then hard reset
            CurveKind::Sawtooth => {
                let phase = t % PERIOD;
                1.0 - phase / PERIOD
            }

            // Falls 1->0 in first half-period, rises 0->1 in second half
            CurveKind::Triangle => {
                let phase = t % PERIOD;
                let half = PERIOD / 2.0;
                if phase < half {
                    1.0 - phase / half
                } else {
                    (phase - half) / half
                }
            }

            // 1.0 for first half-period, 0.0 for second — no middle ground
            CurveKind::Square => {
                let phase = t % PERIOD;
                if phase < PERIOD / 2.0 { 1.0 } else { 0.0 }
            }

            // e^(-k*t) — starts at 1.0, decays toward 0 with no recovery.
            // Uses phase_elapsed so it always resets to 1.0 on personality flip.
            CurveKind::Exponential => (-DECAY_K * t).exp(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solace(kind: CurveKind) -> ProbabilityCurve { kind.build(BipolarMode::Solace) }
    fn eclipse(kind: CurveKind) -> ProbabilityCurve { kind.build(BipolarMode::Eclipse) }

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
        for kind in CurveKind::all() {
            let v = solace(kind).evaluate(0.0);
            assert!(v >= 0.5, "{kind:?} SOLACE starts at {v}, expected >= 0.5");
        }
    }

    #[test]
    fn eclipse_curves_start_low_or_neutral() {
        for kind in CurveKind::all() {
            let v = eclipse(kind).evaluate(0.0);
            assert!(v <= 0.5, "{kind:?} ECLIPSE starts at {v}, expected <= 0.5");
        }
    }

    #[test]
    fn exponential_starts_at_one_and_decays() {
        let c = solace(CurveKind::Exponential);
        assert!((c.evaluate(0.0) - 1.0).abs() < 1e-10);
        assert!(c.evaluate(0.0) > c.evaluate(100.0));
    }

    #[test]
    fn sawtooth_starts_at_one_and_falls() {
        let c = solace(CurveKind::Sawtooth);
        assert!((c.evaluate(0.0) - 1.0).abs() < 1e-10);
        assert!(c.evaluate(5.0) < c.evaluate(0.0));
    }

    #[test]
    fn triangle_starts_at_one_and_falls_first() {
        let c = solace(CurveKind::Triangle);
        assert!((c.evaluate(0.0) - 1.0).abs() < 1e-10);
        assert!(c.evaluate(2.5) < c.evaluate(0.0));
    }
}
