use common_game::utils::ID;
use std::fmt;

#[derive(Debug)]
pub enum OrchestratorError {
    /// galaxy.txt or planets.toml couldn't be read or parsed
    GalaxyFileError(String),
    PlanetNotFound(ID),
    ExplorerNotFound(ID),
    NotANeighbor { from: ID, to: ID },
    /// A send failed, the other side disconnected, or an ack timed out.
    ChannelError(String),
    /// `start_logic` while already running, or `stop_logic` while stopped.
    InvalidState(String),
    /// A planet's factory returned an error.
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
