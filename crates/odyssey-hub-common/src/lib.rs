uniffi::setup_scaffolding!();

pub mod accessory;
pub mod config;
pub mod device;
pub mod events;
mod hexkeymap;
mod screen_info;

pub use screen_info::ScreenInfo;
