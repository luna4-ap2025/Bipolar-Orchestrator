//! The curves each planet can get. A curve gives a "wobble" between 0 and 1
//! over time (1 = sunray, 0 = asteroid), so every planet has its own rhythm.
//!
//! Hostility then mixes the wobble with its opposite: at 0 the curve is used
//! as it is, at 1 it's flipped, in between it's a lerp. So planets keep their
//! own shape but everything leans toward asteroids as hostility goes up.
//!
//! | Kind          | at t=0 | Shape                                |
//! |---------------|--------|--------------------------------------|
//! | `Sine`        | 0.5    | smooth wave                          |
//! | `Cosine`      | 1.0    | smooth wave                          |
//! | `Sawtooth`    | 1.0    | goes down in a line, then jumps back |
//! | `Triangle`    | 1.0    | goes down, then back up              |
//! | `Square`      | 1.0    | 1 for half the period, then 0        |
//! | `Exponential` | 1.0    | decays toward 0 and stays there      |
//! | `Staircase`   | 1.0    | 4 steps down, then back to the top   |

// In seconds. Changed together with TICK_INTERVAL (now 25s) so a curve still
// takes about the same number of ticks to go around.
const PERIOD: f64 = 1000.0;
const DECAY_K: f64 = 0.0125;

// so Sine/Cosine also do one cycle per PERIOD (plain sin(t) repeats every
// ~6s, which basically made them random from tick to tick)
const ANGULAR_FREQ: f64 = std::f64::consts::TAU / PERIOD;

// Most curves hit 0 at some point, which is a guaranteed asteroid even when
// hostility is 0, and games were ending in under a minute. The floor only
// applies to the wobble, so at full hostility curves still flip all the way
// down to 0 (full Eclipse is supposed to be deadly).
// Was 0.3, raised to 0.45 because even 40-60% asteroid chances wiped out all
// 7 planets really fast.
const WOBBLE_FLOOR: f64 = 0.45;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveKind {
    Sine,
    Cosine,
    Sawtooth,
    Triangle,
    Square,
    Exponential,
    Staircase,
}

impl CurveKind {
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Sine,
            Self::Cosine,
            Self::Sawtooth,
            Self::Triangle,
            Self::Square,
            Self::Exponential,
            Self::Staircase,
        ]
    }

    #[must_use]
    pub fn build(self) -> ProbabilityCurve {
        ProbabilityCurve { kind: self }
    }
}

pub struct ProbabilityCurve {
    pub(super) kind: CurveKind,
}

impl ProbabilityCurve {
    /// Sunray probability at `t` seconds for the given hostility.
    #[must_use]
    pub fn evaluate(&self, t: f64, hostility: f64) -> f64 {
        let wobble = self.evaluate_wobble(t).max(WOBBLE_FLOOR);
        wobble + hostility * (1.0 - 2.0 * wobble)
    }

    fn evaluate_wobble(&self, t: f64) -> f64 {
        match self.kind {
            // (sin + 1) / 2 so it stays in 0..1
            CurveKind::Sine => f64::midpoint((t * ANGULAR_FREQ).sin(), 1.0),

            CurveKind::Cosine => f64::midpoint((t * ANGULAR_FREQ).cos(), 1.0),

            CurveKind::Sawtooth => {
                let phase = t % PERIOD;
                1.0 - phase / PERIOD
            }

            CurveKind::Triangle => {
                let phase = t % PERIOD;
                let half = PERIOD / 2.0;
                if phase < half {
                    1.0 - phase / half
                } else {
                    (phase - half) / half
                }
            }

            CurveKind::Square => {
                let phase = t % PERIOD;
                if phase < PERIOD / 2.0 { 1.0 } else { 0.0 }
            }

            // e^(-kt), never comes back up (the floor stops it at 0.45)
            CurveKind::Exponential => (-DECAY_K * t).exp(),

            // 1.0, 0.75, 0.5, 0.25, then back to 1.0
            CurveKind::Staircase => {
                let phase = t % PERIOD;
                let step = (phase / (PERIOD / 4.0)).floor();
                1.0 - step * 0.25
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calm(kind: CurveKind) -> ProbabilityCurve {
        kind.build()
    }

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
            assert!(
                v >= 0.5,
                "{kind:?} at hostility=0 starts at {v}, expected >= 0.5"
            );
        }
    }

    #[test]
    fn full_hostility_starts_low_or_neutral() {
        for kind in CurveKind::all() {
            let v = calm(kind).evaluate(0.0, 1.0);
            assert!(
                v <= 0.5,
                "{kind:?} at hostility=1 starts at {v}, expected <= 0.5"
            );
        }
    }

    #[test]
    fn wobble_floor_prevents_guaranteed_death_at_zero_hostility() {
        for kind in CurveKind::all() {
            let c = calm(kind);
            for t in [0.0_f64, 10.0, 40.0, 80.0, 120.0, 160.0, 300.0] {
                let v = c.evaluate(t, 0.0);
                assert!(
                    v >= WOBBLE_FLOOR - 1e-10,
                    "{kind:?} at t={t}, hostility=0 dropped to {v}, below the floor"
                );
            }
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
    fn staircase_starts_at_one_and_steps_down() {
        let c = calm(CurveKind::Staircase);
        let quarter = PERIOD / 4.0;
        assert!((c.evaluate(0.0, 0.0) - 1.0).abs() < 1e-10);
        assert!((c.evaluate(quarter, 0.0) - 0.75).abs() < 1e-10);
        assert!((c.evaluate(quarter * 2.0, 0.0) - 0.5).abs() < 1e-10);
        // last step is 0.25, but the floor raises it
        assert!((c.evaluate(quarter * 3.0, 0.0) - WOBBLE_FLOOR).abs() < 1e-10);
        assert!((c.evaluate(PERIOD, 0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn intermediate_hostility_lerps_between_wobble_and_inverse() {
        let c = calm(CurveKind::Cosine);
        let t = 0.0;
        let wobble = c.evaluate(t, 0.0);
        let mid = c.evaluate(t, 0.5);
        assert!(
            (mid - 0.5).abs() < 1e-10,
            "midpoint hostility should read neutral, got {mid}"
        );
        assert!(wobble > mid);
    }
}
