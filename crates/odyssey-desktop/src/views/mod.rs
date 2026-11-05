//! The app's pages.

mod accessories;
pub(crate) mod calibrate;
mod device_label;
mod home;
pub(crate) mod pairing;
mod settings;
pub(crate) mod zero;

pub(crate) use accessories::AccessoriesView;
pub(crate) use calibrate::CalibrateView;
pub(crate) use device_label::device_label;
pub(crate) use home::{HomeEvent, HomeView};
pub(crate) use pairing::PairingView;
pub(crate) use settings::SettingsView;
