//! The galaxy: which planets exist and who is next to who. Reads galaxy.txt
//! and planets.toml.

pub mod parser;
pub mod planet_config;
pub mod topology;

pub use planet_config::PlanetConfigMap;
pub use topology::Topology;
