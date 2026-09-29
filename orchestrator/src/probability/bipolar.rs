//! Which personality is in control. It's only a label (for logs and the GUI),
//! the probabilities use `hostility` directly.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BipolarMode {
    Solace,
    Eclipse,
}

impl BipolarMode {
    /// Eclipse from 0.5 up, Solace below.
    #[must_use]
    pub fn from_hostility(hostility: f64) -> Self {
        if hostility >= 0.5 {
            Self::Eclipse
        } else {
            Self::Solace
        }
    }

    #[must_use]
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
