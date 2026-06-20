//! # Explorer routing
//!
//! Implements the three-step protocol for moving an explorer from one planet
//! to another:
//!
//! 1. Send [`OutgoingExplorerRequest`] to the *current* planet → wait for ack.
//! 2. Create a new `ExplorerToPlanet` channel pair. Send [`IncomingExplorerRequest`]
//!    to the *destination* planet (with the new sender) → wait for ack.
//! 3. Send [`MoveToPlanet`] to the explorer (with the new sender) → wait for ack.
//!
//! This module is used both by the autonomous logic loop and by the synchronous API.
//!
//! ## Owner: Vale

pub mod move_explorer;

pub use move_explorer::execute as move_explorer;
