//! Moving explorers between planets (see `move_explorer.rs`). Used by both
//! the logic loop and the API.

pub mod move_explorer;

pub use move_explorer::execute as move_explorer;
