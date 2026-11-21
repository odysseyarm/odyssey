//! Release signatures for firmware images: the signed message, the trusted
//! public keys and the verifier, shared by device firmware and host tooling so
//! both build the message the same way.
//!
//! The signing flow and key custody are described in the "Signed releases"
//! section of `firmware/manifests/README.md`.
#![no_std]

mod hash;
mod keys;
mod message;
mod verify;

pub use hash::Sha256;
pub use keys::{KEY_SLOTS, PublicKey, RELEASE_KEYS, trusted_keys};
pub use message::{Component, MESSAGE_LEN, message};
pub use verify::{SIGNATURE_LEN, verify, verify_production};
