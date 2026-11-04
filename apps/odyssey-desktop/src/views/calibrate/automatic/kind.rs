//! What kind of display is being calibrated and what its size is known to be.
//!
//! A flat screen (TV or monitor) reports its physical size in its EDID, which
//! beats the depth camera's measurement as a scale anchor. A projector's EDID
//! says nothing true about the picture, so only the camera can size it. Every
//! decision about that lives here as plain functions: what the lookup found,
//! which kind to preselect, and which size and edge rule the session gets.

mod finding;
mod size_plan;
#[cfg(test)]
mod tests;

pub(crate) use finding::Finding;
pub(crate) use odyssey_hub_common::config::DisplayKind;
pub(crate) use size_plan::{SizeOrigin, SizePlan, default_kind, plan, source_label};
