//! # Bipolar mode tracker
//!
//! Tracks whether the galaxy is currently in **Normal** or **Flipped** mode.
//! Every time a planet dies, the mode toggles. This is the "bipolar" mechanic:
//! the galaxy alternates between being benign (many sunrays) and hostile (many
//! asteroids) each time a planet is destroyed.
//!
//! ## Owner: Vale

/// The current phase of the bipolar mechanic.
///
/// `Normal`  → curves run as assigned (early game: many sunrays).
/// `Flipped` → every curve is inverted (`1.0 - value`), making the galaxy
///             more hostile (many asteroids).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BipolarMode {
    Normal,
    Flipped,
}

impl BipolarMode {
    /// Returns the next mode (toggles between `Normal` and `Flipped`).
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Flipped,
            Self::Flipped => Self::Normal,
        }
    }

    /// Returns `true` if the mode is currently flipped.
    pub fn is_flipped(self) -> bool {
        matches!(self, Self::Flipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_correctly() {
        let m = BipolarMode::Normal;
        assert_eq!(m.next(), BipolarMode::Flipped);
        assert_eq!(m.next().next(), BipolarMode::Normal);
    }
}
