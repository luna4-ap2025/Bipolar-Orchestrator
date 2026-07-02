//! # Probability curves
//!
//! Each [`CurveKind`] maps phase time `t` (seconds since the last planet
//! death) to a local "wobble" value in `[0.0, 1.0]` — this is each planet's
//! own personality, independent of the galaxy's mood.
//!
//! That wobble is then blended against its own inverse by the registry's
//! global `hostility` value (see [`super::ProbabilityRegistry`]): at
//! `hostility = 0.0` the curve reads as-is (calm), at `hostility = 1.0` it
//! reads fully inverted (hostile), and in between it's a straight lerp. So
//! every planet keeps its individual shape while the whole galaxy leans
//! further toward asteroids as hostility climbs.
//!
//! | Kind          | t=0 wobble | Shape                            |
//! |---------------|------------|-----------------------------------|
//! | `Sine`        | 0.5        | smooth oscillation, neutral open |
//! | `Cosine`      | 1.0        | smooth oscillation, full open    |
//! | `Sawtooth`    | 1.0        | linear fall then hard reset      |
//! | `Triangle`    | 1.0        | falls then rises, symmetric      |
//! | `Square`      | 1.0        | hard binary alternation          |
//! | `Exponential` | 1.0        | decays toward 0, no recovery     |

// Scaled 4x alongside `logic::TICK_INTERVAL` (1s -> 4s) so curves still swing
// over the same number of ticks, just more slowly in wall-clock time.
const PERIOD: f64 = 40.0;
const DECAY_K: f64 = 0.0125;

// Angular frequency for Sine/Cosine so they complete one full cycle every
// `PERIOD` seconds, matching Sawtooth/Triangle/Square. Raw `t.sin()`/`t.cos()`
// (frequency 1 rad/s) would instead cycle every ~6.28s regardless of `PERIOD`,
// making a Sine/Cosine planet's odds swing almost randomly between 4s ticks.
const ANGULAR_FREQ: f64 = std::f64::consts::TAU / PERIOD;

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

    pub fn build(self) -> ProbabilityCurve {
        ProbabilityCurve { kind: self }
    }
}

pub struct ProbabilityCurve {
    pub(super) kind: CurveKind,
}

impl ProbabilityCurve {
    /// Sunray probability at `t` seconds into the current phase, blended
    /// against its own inverse by `hostility` (`0.0` = pure wobble, `1.0` =
    /// fully inverted).
    #[must_use]
    pub fn evaluate(&self, t: f64, hostility: f64) -> f64 {
        let wobble = self.evaluate_wobble(t);
        wobble + hostility * (1.0 - 2.0 * wobble)
    }

    fn evaluate_wobble(&self, t: f64) -> f64 {
        match self.kind {
            // (sin(t) + 1) / 2 — oscillates [0, 1] once per PERIOD, starts at 0.5
            CurveKind::Sine => ((t * ANGULAR_FREQ).sin() + 1.0) / 2.0,

            // (cos(t) + 1) / 2 — oscillates [0, 1] once per PERIOD, starts at 1.0
            CurveKind::Cosine => ((t * ANGULAR_FREQ).cos() + 1.0) / 2.0,

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
            // Uses phase_elapsed so it always resets to 1.0 on planet death.
            CurveKind::Exponential => (-DECAY_K * t).exp(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calm(kind: CurveKind) -> ProbabilityCurve { kind.build() }

    #[test]
    fn all_curves_return_values_in_unit_interval() {
        for kind in CurveKind::all() {
            for t in [0.0_f64, 1.0, 5.0, 10.0, 100.0] {
                let v = calm(kind).evaluate(t, 0.0);
                assert!(
                    (0.0..=1.0).contains(&v),
                    "{kind:?} at t={t} returned {v} outside [0,1]"
                );
            }
        }
    }

    #[test]
    fn full_hostility_inverts_wobble() {
        for kind in CurveKind::all() {
            let t = 3.7_f64;
            let calm_v = calm(kind).evaluate(t, 0.0);
            let hostile_v = calm(kind).evaluate(t, 1.0);
            assert!(
                (calm_v + hostile_v - 1.0).abs() < 1e-10,
                "{kind:?}: calm={calm_v} + hostile={hostile_v} should sum to 1"
            );
        }
    }

    #[test]
    fn zero_hostility_starts_high_or_neutral() {
        for kind in CurveKind::all() {
            let v = calm(kind).evaluate(0.0, 0.0);
            assert!(v >= 0.5, "{kind:?} at hostility=0 starts at {v}, expected >= 0.5");
        }
    }

    #[test]
    fn full_hostility_starts_low_or_neutral() {
        for kind in CurveKind::all() {
            let v = calm(kind).evaluate(0.0, 1.0);
            assert!(v <= 0.5, "{kind:?} at hostility=1 starts at {v}, expected <= 0.5");
        }
    }

    #[test]
    fn exponential_starts_at_one_and_decays() {
        let c = calm(CurveKind::Exponential);
        assert!((c.evaluate(0.0, 0.0) - 1.0).abs() < 1e-10);
        assert!(c.evaluate(0.0, 0.0) > c.evaluate(100.0, 0.0));
    }

    #[test]
    fn sawtooth_starts_at_one_and_falls() {
        let c = calm(CurveKind::Sawtooth);
        assert!((c.evaluate(0.0, 0.0) - 1.0).abs() < 1e-10);
        assert!(c.evaluate(5.0, 0.0) < c.evaluate(0.0, 0.0));
    }

    #[test]
    fn triangle_starts_at_one_and_falls_first() {
        let c = calm(CurveKind::Triangle);
        assert!((c.evaluate(0.0, 0.0) - 1.0).abs() < 1e-10);
        assert!(c.evaluate(2.5, 0.0) < c.evaluate(0.0, 0.0));
    }

    #[test]
    fn intermediate_hostility_lerps_between_wobble_and_inverse() {
        let c = calm(CurveKind::Cosine);
        let t = 0.0;
        let wobble = c.evaluate(t, 0.0);
        let mid = c.evaluate(t, 0.5);
        assert!((mid - 0.5).abs() < 1e-10, "midpoint hostility should read neutral, got {mid}");
        assert!(wobble > mid);
    }
}
