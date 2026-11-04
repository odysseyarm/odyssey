//! The fullscreen window that shows the calibration dot pattern on the target
//! monitor.
//!
//! The pattern is an SVG in the pattern's own unit space (`Pattern::dots`), so
//! it fills whatever resolution the monitor has. The window lives exactly as
//! long as its [`PatternDisplay`]: dropping the display closes the window.

mod display;
mod mode;
mod pictures;
mod placement;
mod screen;

pub(crate) use display::{PatternDisplay, open};
pub(crate) use mode::{PatternEvent, PatternLoss, PatternMode};
pub(crate) use placement::covers_window;
