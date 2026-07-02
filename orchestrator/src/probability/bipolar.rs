//! # Bipolar mode label
//!
//! `BipolarMode` is a *derived* label for which orchestrator personality is
//! dominant right now — it no longer drives probability math directly. The
//! real driver is [`super::ProbabilityRegistry`]'s continuous `hostility`
//! value; this enum just buckets that value into SOLACE (< 0.5, nurturing,
//! sunray-biased) or ECLIPSE (>= 0.5, destructive, asteroid-biased) for logs
//! and GUI text.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BipolarMode {
    Solace,
    Eclipse,
}

impl BipolarMode {
    /// Buckets a continuous hostility value in `[0.0, 1.0]` into a dominant label.
    pub fn from_hostility(hostility: f64) -> Self {
        if hostility >= 0.5 { Self::Eclipse } else { Self::Solace }
    }

    /// Returns `true` if ECLIPSE is currently dominant.
    pub fn is_eclipse(self) -> bool {
        matches!(self, Self::Eclipse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_by_threshold() {
        assert_eq!(BipolarMode::from_hostility(0.0), BipolarMode::Solace);
        assert_eq!(BipolarMode::from_hostility(0.49), BipolarMode::Solace);
        assert_eq!(BipolarMode::from_hostility(0.5), BipolarMode::Eclipse);
        assert_eq!(BipolarMode::from_hostility(1.0), BipolarMode::Eclipse);
    }
}
