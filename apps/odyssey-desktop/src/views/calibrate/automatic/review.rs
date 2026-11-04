//! Step 5: the result. The camera picture with the fitted screen outline and
//! marker positions, the measured size, a green/amber/red verdict, and Save.
//! Then the success screen with what to do next.

mod notices;
mod result_overlay;
mod review_step;
mod saved_step;

pub(super) use review_step::ReviewStep;
pub(super) use saved_step::SavedStep;
