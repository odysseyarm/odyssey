//! The app's routed pages.

// dioxus-rsx 0.7 expands every event handler (`onclick: ...`) to the qualified
// path `dioxus_elements::events::onclick::...` spanned at the user's code, which
// rustc then reports as an unnecessary qualification. The markup cannot avoid it.
#![expect(
    unused_qualifications,
    reason = "false positive on rsx! event-handler expansion"
)]

mod accessories;
pub(crate) mod calibrate;
mod device_label;
mod home;
pub(crate) mod pairing;
mod settings;
mod zero;

pub(crate) use accessories::Accessories;
pub(crate) use calibrate::Calibrate;
pub(crate) use calibrate::automatic::CalibrateAutomatic;
pub(crate) use device_label::device_label;
pub(crate) use home::Home;
pub(crate) use pairing::Pairing;
pub(crate) use settings::Settings;
