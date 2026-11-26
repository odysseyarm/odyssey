//! Release signatures for firmware images: the signed message, the trusted
//! public keys and the verifier, shared by device firmware and host tooling so
//! both build the message the same way.
//!
//! The signing flow and key custody are described in the "Signed releases"
//! section of `firmware/manifests/README.md`.
#![no_std]

mod dfu;
mod hash;
mod keys;
mod message;
mod trust;
mod verify;

pub use dfu::{DFU_HEADER_LEN, DfuError, DfuVerifier, dfu_header};
pub use hash::Sha256;
#[cfg(any(test, feature = "dev-signing-key"))]
pub use keys::DEV_KEY;
pub use keys::{KEY_SLOTS, PublicKey, RELEASE_KEYS, trusted_keys};
pub use message::{Component, MESSAGE_LEN, message};
pub use trust::{
    MAX_SIGNATURES, POLICY_RECORD_MAX, RECORD_HEADER_LEN, RootSet, SIGNATURE_ENTRY_LEN,
    TRUST_LIST_LEN, TrustList, TrustListError,
};
pub use verify::{SIGNATURE_LEN, verify, verify_production, verify_with_keys};
