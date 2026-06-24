//! # Galaxy module
//!
//! Owns the topology of the galaxy (which planets are neighbors of which) and
//! provides the parser for the galaxy initialization file.
//!
//! ## Responsibilities
//! - Parse the galaxy init file into a [`Topology`].
//! - Track which planets are still alive.
//! - Answer neighbor queries used by the router and the logic loop.
//!
//! ## Owner: Vale

pub mod parser;
pub mod planet_config;
pub mod topology;

pub use planet_config::PlanetConfigMap;
pub use topology::Topology;
