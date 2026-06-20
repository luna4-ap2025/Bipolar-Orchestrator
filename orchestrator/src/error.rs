//! # Error types for the Bipolar Orchestrator
//!
//! All fallible orchestrator operations return [`OrchestratorError`].

use common_game::utils::ID;
use std::fmt;

/// Unified error type for all orchestrator operations.
#[derive(Debug)]
pub enum OrchestratorError {
    /// The galaxy initialization file could not be read or parsed.
    GalaxyFileError(String),
    /// A planet with the given ID was not found in the galaxy.
    PlanetNotFound(ID),
    /// An explorer with the given ID was not found.
    ExplorerNotFound(ID),
    /// The destination planet is not a neighbor of the current planet.
    NotANeighbor { from: ID, to: ID },
    /// A channel send/receive operation failed (the other end disconnected).
    ChannelError(String),
    /// The game logic is already running when `start_logic` was called, or
    /// already stopped when `stop_logic` was called.
    InvalidState(String),
    /// A planet factory function returned an error during construction.
    PlanetConstructionError(String),
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GalaxyFileError(msg) => write!(f, "Galaxy file error: {msg}"),
            Self::PlanetNotFound(id) => write!(f, "Planet {id} not found"),
            Self::ExplorerNotFound(id) => write!(f, "Explorer {id} not found"),
            Self::NotANeighbor { from, to } => {
                write!(f, "Planet {to} is not a neighbor of planet {from}")
            }
            Self::ChannelError(msg) => write!(f, "Channel error: {msg}"),
            Self::InvalidState(msg) => write!(f, "Invalid state: {msg}"),
            Self::PlanetConstructionError(msg) => write!(f, "Planet construction error: {msg}"),
        }
    }
}

impl std::error::Error for OrchestratorError {}
