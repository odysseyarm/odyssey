/// Number of trusted production key slots.
///
/// Since: Unreleased.
pub const KEY_SLOTS: usize = 4;

/// A raw Ed25519 public key.
///
/// Since: Unreleased.
pub type PublicKey = [u8; 32];

/// An all-zero key marks an unused slot. It decodes as a valid low-order curve
/// point, so the verifier must skip it explicitly rather than rely on decoding.
pub(crate) const EMPTY_KEY: PublicKey = [0; 32];

/// Production release keys, one per slot; all-zero slots are empty.
///
/// Slot 0 holds the first publisher key; the rest are empty until more publishers exist. Fill them in here
/// when they do, as described in `firmware/manifests/README.md`.
///
/// Since: Unreleased.
pub const RELEASE_KEYS: [PublicKey; KEY_SLOTS] = [
    // matthew@ampotato
    [
        0x96, 0x91, 0xa6, 0x72, 0x67, 0x10, 0x7a, 0x9b, 0x25, 0x1c, 0x4a, 0xa6, 0x0b, 0xec, 0xdc,
        0x9c, 0x9e, 0x40, 0x84, 0x69, 0xd4, 0x98, 0x4d, 0x30, 0xe3, 0xae, 0xe1, 0xb6, 0x68, 0x7d,
        0x9b, 0x7f,
    ],
    EMPTY_KEY,
    EMPTY_KEY,
    EMPTY_KEY,
];

/// Public half of the development key. Its seed is public (see `crates/xtask`),
/// so builds that trust it can be forged by anyone and must never ship.
#[cfg(any(test, feature = "dev-signing-key"))]
pub(crate) const DEV_KEY: PublicKey = [
    0xe4, 0xc9, 0x25, 0x28, 0xd6, 0xf0, 0x39, 0xb4, 0x1c, 0x56, 0x45, 0x9c, 0x06, 0x6e, 0xf8, 0x72,
    0x0e, 0x5f, 0x8b, 0xcf, 0x40, 0x50, 0x47, 0x74, 0xe8, 0x05, 0x95, 0x97, 0xb2, 0x2c, 0xa2, 0x1e,
];

#[cfg(feature = "dev-signing-key")]
const DEV_KEYS: &[PublicKey] = &[DEV_KEY];
#[cfg(not(feature = "dev-signing-key"))]
const DEV_KEYS: &[PublicKey] = &[];

/// Keys this build trusts: [`RELEASE_KEYS`] plus the development key when the
/// `dev-signing-key` feature is on. Empty slots are included; the verifier
/// skips them.
///
/// Since: Unreleased.
pub fn trusted_keys() -> impl Iterator<Item = &'static PublicKey> {
    RELEASE_KEYS.iter().chain(DEV_KEYS)
}
