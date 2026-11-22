//! SHA-256, from Nordic's Oberon library on the nRF5340 firmware (feature `oberon`) and from
//! `sha2` everywhere else. Oberon is smaller than `sha2`, which matters in the app flash banks.

/// Incremental SHA-256.
///
/// Since: Unreleased.
#[cfg(not(feature = "oberon"))]
#[derive(Debug, Default)]
pub struct Sha256(sha2::Sha256);

#[cfg(not(feature = "oberon"))]
impl Sha256 {
    /// Starts an empty hash.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds `bytes` into the hash.
    ///
    /// Since: Unreleased.
    pub fn update(&mut self, bytes: &[u8]) {
        sha2::Digest::update(&mut self.0, bytes);
    }

    /// Returns the digest of everything fed so far.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub fn finalize(self) -> [u8; 32] {
        sha2::Digest::finalize(self.0).into()
    }
}

/// Incremental SHA-256.
///
/// Since: Unreleased.
#[cfg(feature = "oberon")]
pub struct Sha256(nrfxlib_sys::ocrypto_sha256_ctx);

#[cfg(feature = "oberon")]
impl core::fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Sha256")
    }
}

#[cfg(feature = "oberon")]
#[expect(unsafe_code, reason = "calls the Oberon C library")]
impl Sha256 {
    /// Starts an empty hash.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: the context is plain integers, and `ocrypto_sha256_init` sets it up.
        let mut context: nrfxlib_sys::ocrypto_sha256_ctx = unsafe { core::mem::zeroed() };
        // SAFETY: `context` is a valid, exclusively borrowed context.
        unsafe { nrfxlib_sys::ocrypto_sha256_init(&mut context) };
        Self(context)
    }

    /// Feeds `bytes` into the hash.
    ///
    /// Since: Unreleased.
    pub fn update(&mut self, bytes: &[u8]) {
        // SAFETY: the context was initialised by `new`, and the pointer and length describe `bytes`.
        unsafe { nrfxlib_sys::ocrypto_sha256_update(&mut self.0, bytes.as_ptr(), bytes.len()) };
    }

    /// Returns the digest of everything fed so far.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub fn finalize(mut self) -> [u8; 32] {
        let mut digest = [0; 32];
        // SAFETY: the context was initialised by `new`, and `digest` holds the 32 output bytes.
        unsafe { nrfxlib_sys::ocrypto_sha256_final(&mut self.0, digest.as_mut_ptr()) };
        digest
    }
}

#[cfg(feature = "oberon")]
impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// The digest of `bytes` in one call.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> [u8; 32] {
        let mut hash = Self::new();
        hash.update(bytes);
        hash.finalize()
    }
}
