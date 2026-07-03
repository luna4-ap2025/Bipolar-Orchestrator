//! # Bipolar Orchestrator - library root
//!
//! Re-exports all public modules so integration tests and the visualizer
//! can depend on this crate as a library.

pub mod ack;
pub mod api;
pub mod builder;
pub mod error;
pub mod explorer;
pub mod galaxy;
pub mod logic;
pub mod planet;
pub mod probability;
pub mod routing;
pub mod snapshot;
