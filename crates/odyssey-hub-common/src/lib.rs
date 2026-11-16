uniffi::setup_scaffolding!();

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub mod accessory;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub mod config;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub mod device;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub mod events;
/// Since: Odyssey Desktop (unreleased).
pub mod firmware_version;
mod hexkeymap;
mod screen_info;

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub use screen_info::ScreenInfo;
