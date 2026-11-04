//! The Orbbec camera SDK on disk: where the app looks for it, whether the copy it
//! found is the one this build is tested with, and in the online build how a copy
//! gets there.
//!
//! The SDK is optional. It is a native library the automatic calibration wizard
//! loads at run time (see `odyssey_autocalib::orbbec::runtime`), and it can come
//! from two places:
//!
//! * the offline installer, which carries it in `orbbec-sdk\` beside the app, or
//! * a download the user asked for, kept in `orbbec-sdk\` under the app's data
//!   folder. Only builds with the `sdk-download` feature contain that code.
//!
//! The version, files and hashes both paths use are in `orbbec-sdk.json`, which
//! the Windows release workflow reads too, so there is one place to bump.
//!
//! Once the library is loaded it stays mapped until the app exits, and Windows will
//! not rename or delete files that are mapped. Replacing or removing a copy that is
//! in use therefore moves it aside in one step, which either works or changes
//! nothing, and otherwise leaves a marker for the next start to act on.
//! Only a refused move of the current copy is deferred: other filesystem errors
//! are reported. Staging is invalidated before cleanup, and removal intent stays
//! on disk until cleanup succeeds, so interrupted work cannot reinstall a copy
//! the user removed. A later completed download supersedes an earlier removal.
//! Downloads have a deadline, are bounded before allocation and while streaming,
//! and must match the pinned archive size and hashes. Archive files are written
//! only to fresh, relative destinations without overwriting files.

mod advisory;
mod delivery;
#[cfg(feature = "sdk-download")]
mod download;
mod locations;
mod managed_copy;
mod pin_file;

pub(crate) use advisory::{Advisory, current_advisory};
pub(crate) use delivery::{Delivery, delivery};
#[cfg(feature = "sdk-download")]
pub(crate) use download::{Installed, Phase, install};
pub(crate) use locations::search_dir;
pub(crate) use managed_copy::{
    Removal, finish_pending, has_managed_copy, removal_pending, remove_managed_copy, update_pending,
};
#[cfg(feature = "sdk-download")]
pub(crate) use pin_file::pin;
