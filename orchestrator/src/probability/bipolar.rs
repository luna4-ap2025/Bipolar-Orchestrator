//! # Bipolar mode tracker
//!
//! Tracks whether the galaxy is currently under **SOLACE** or **ECLIPSE**.
//! Every time a planet dies, the personality flips. This is the bipolar mechanic:
//! the galaxy alternates between nurturing (SOLACE, high sunray probability) and
//! destructive (ECLIPSE, high asteroid probability) each time a planet is destroyed.
//!
//! ## Owner: Vale

/// The active orchestrator personality.
///
/// `Solace`  → curves run as assigned. Sunray-leaning. She nurtures.
/// `Eclipse` → every curve is inverted (`1.0 - value`). Asteroid-leaning. She destroys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BipolarMode {
    Solace,
    Eclipse,
}

impl BipolarMode {
    /// Flips to the other personality. Called on every planet death.
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
