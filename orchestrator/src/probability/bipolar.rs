//! # Bipolar mode tracker
//!
//! Tracks which orchestrator personality is currently active: SOLACE (nurturing,
//! sunray-biased) or ECLIPSE (destructive, asteroid-biased). Flips on every
//! planet death.

/// The active orchestrator personality.
///
/// `Solace`: curves run as assigned, sunray probability is high.
/// `Eclipse`: every curve is inverted (`1.0 - value`), asteroid probability is high.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BipolarMode {
    Solace,
    Eclipse,
}

impl BipolarMode {
    /// Returns the opposite personality.
    pub fn next(self) -> Self {
        match self {
            Self::Solace => Self::Eclipse,
            Self::Eclipse => Self::Solace,
        }
    }

    /// Returns `true` if ECLIPSE is currently in control.
    pub fn is_eclipse(self) -> bool {
        matches!(self, Self::Eclipse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_correctly() {
        let m = BipolarMode::Solace;
        assert_eq!(m.next(), BipolarMode::Eclipse);
        assert_eq!(m.next().next(), BipolarMode::Solace);
    }
}
