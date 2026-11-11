#![no_std]

mod output_state;
mod power_button;
/// Since: Odyssey Desktop 0.7.4.
pub mod security;
mod valid_vectors;

/// Since: Odyssey Desktop 0.7.4.
pub use output_state::{Led, OutputState};
/// Since: Odyssey Desktop 0.7.4.
pub use power_button::{ButtonAction, PowerButton};
/// Since: Odyssey Desktop 0.7.4.
pub use valid_vectors::valid_vectors;
