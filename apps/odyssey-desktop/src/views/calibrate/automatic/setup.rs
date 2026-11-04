//! Step 1: what is being calibrated, which screen, which camera, and how big
//! it is (known from the display, typed in, or left to the camera).

mod camera_picker;
mod flat_size;
mod kind_chooser;
mod projector_size;
mod screen_number_picker;
mod setup_step;
mod size_box;
mod size_section;

pub(super) use setup_step::SetupStep;
