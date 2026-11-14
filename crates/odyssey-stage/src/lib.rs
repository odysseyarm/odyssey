//! Authoring model for simulated Odyssey scenes: a display with real-world
//! flaws, the devices placed around it, light and radio conditions, and the
//! patterns that move them.
//!
//! Coordinates follow the scene format: the display lies in the plane `z = 0`
//! with +X right and +Y down, and a viewer stands at negative Z looking toward
//! +Z. Everything here is a model for comparing setups. None of the radio or
//! light numbers are measurements of Odyssey hardware.

mod compiled;
mod error;
mod live;
mod scene;
mod stage;

/// Since: Odyssey Desktop 0.7.5.
pub mod display;
/// Since: Odyssey Desktop 0.7.5.
pub mod light;
/// Since: Odyssey Desktop 0.7.5.
pub mod pattern;
/// Since: Odyssey Desktop 0.7.5.
pub mod radio;

/// Since: Odyssey Desktop 0.7.5.
pub use compiled::{Compiled, LinkReport, Pose};
/// Since: Odyssey Desktop 0.7.5.
pub use error::StageError;
/// Since: Odyssey Desktop 0.7.5.
pub use live::{Cue, SimFeed, scenario};
/// Since: Odyssey Desktop 0.7.5.
pub use stage::{
    Binding, DISPLAY_PRESET_NAMES, Device, DeviceKind, SCHEMA, Stage, Tracker, display_preset,
};
