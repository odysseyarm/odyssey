#![no_std]
#![expect(unsafe_code, reason = "CC312 and Oberon cross the C ABI")]

mod allocator;

use core::cell::RefCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

use critical_section::Mutex;

const HASH_OK: i32 = 0;
const HASH_UNSUPPORTED: i32 = -134;
const HASH_BAD_STATE: i32 = -137;
const HASH_BUFFER_TOO_SMALL: i32 = -138;
const HASH_FAILURE: i32 = -147;
const SHA512_SIZE: usize = 64;

static VERIFY_LOCKED: AtomicBool = AtomicBool::new(false);
static HASH_CONTEXT: Mutex<RefCell<Option<MaybeUninit<nrfxlib_sys::ocrypto_sha512_ctx>>>> =
    Mutex::new(RefCell::new(None));

/// Reports serialized access or CryptoCell failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Busy,
    EmptyMessage,
    Vendor(i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("CryptoCell verifier busy"),
            Self::EmptyMessage => f.write_str("CryptoCell requires a non-empty message"),
            Self::Vendor(code) => write!(f, "CryptoCell returned status {code}"),
        }
    }
}

impl core::error::Error for Error {}

struct VerifyGuard;

impl VerifyGuard {
    fn acquire() -> Result<Self, Error> {
        VERIFY_LOCKED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| Error::Busy)
    }
}

impl Drop for VerifyGuard {
    fn drop(&mut self) {
        VERIFY_LOCKED.store(false, Ordering::Release);
    }
}

/// Verify a non-empty Ed25519 message using CryptoCell and Oberon SHA-512.
///
/// The SDK rejects empty messages, including valid RFC 8032 empty-message signatures.
/// Invalid signatures can produce either `Ok(false)` or a vendor error; only `Ok(true)`
/// authenticates a message. Access is synchronous and serialized.
///
/// # Errors
///
/// Returns `EmptyMessage`, `Busy`, or the SDK's non-success status in `Vendor`.
pub fn verify(message: &[u8], key: &[u8; 32], sig: &[u8; 64]) -> Result<bool, Error> {
    if message.is_empty() {
        return Err(Error::EmptyMessage);
    }
    let _guard = VerifyGuard::acquire()?;
    // SAFETY: `message`, `key`, and `sig` stay alive for synchronous C reads; C does not retain them.
    let status = unsafe {
        cc312_verify_ed25519(message.as_ptr(), message.len(), key.as_ptr(), sig.as_ptr())
    };
    match status {
        0 => Ok(true),
        -149 | -135 => Ok(false),
        code => Err(Error::Vendor(code)),
    }
}

// SAFETY: C shim accepts borrowed buffers only for each synchronous call.
unsafe extern "C" {
    fn cc312_verify_ed25519(
        message: *const u8,
        message_length: usize,
        key: *const u8,
        signature: *const u8,
    ) -> i32;
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_setup(supported: bool) -> i32 {
    if !supported {
        return HASH_UNSUPPORTED;
    }
    critical_section::with(|cs| {
        let mut slot = HASH_CONTEXT.borrow(cs).try_borrow_mut().ok()?;
        if slot.is_some() {
            return Some(HASH_BAD_STATE);
        }
        let context = slot.insert(MaybeUninit::uninit());
        // SAFETY: slot holds storage with the SDK context's size and alignment.
        unsafe { nrfxlib_sys::ocrypto_sha512_init(context.as_mut_ptr()) };
        Some(HASH_OK)
    })
    .unwrap_or(HASH_BAD_STATE)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_update(input: *const u8, input_length: usize) -> i32 {
    if input_length != 0 && input.is_null() {
        return HASH_BAD_STATE;
    }
    critical_section::with(|cs| {
        let mut slot = HASH_CONTEXT.borrow(cs).try_borrow_mut().ok()?;
        let Some(context) = slot.as_mut() else {
            return Some(HASH_BAD_STATE);
        };
        if input_length != 0 {
            // SAFETY: non-null input spans `input_length` bytes from synchronous CC312 callback.
            unsafe {
                nrfxlib_sys::ocrypto_sha512_update(context.as_mut_ptr(), input, input_length)
            };
        }
        Some(HASH_OK)
    })
    .unwrap_or(HASH_BAD_STATE)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_finish(
    hash: *mut u8,
    hash_size: usize,
    hash_length: *mut usize,
) -> i32 {
    if hash_size < SHA512_SIZE {
        return HASH_BUFFER_TOO_SMALL;
    }
    if hash.is_null() || hash_length.is_null() {
        return HASH_BAD_STATE;
    }
    critical_section::with(|cs| {
        let mut slot = HASH_CONTEXT.borrow(cs).try_borrow_mut().ok()?;
        let Some(context) = slot.as_mut() else {
            return Some(HASH_BAD_STATE);
        };
        // SAFETY: setup initialized context; C validated non-null output pointers and capacity.
        unsafe { nrfxlib_sys::ocrypto_sha512_final(context.as_mut_ptr(), hash) };
        // SAFETY: C supplied a valid writable length pointer for this synchronous call.
        unsafe { hash_length.write(SHA512_SIZE) };
        *slot = None;
        Some(HASH_OK)
    })
    .unwrap_or(HASH_BAD_STATE)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_abort() -> i32 {
    critical_section::with(|cs| {
        let mut slot = HASH_CONTEXT.borrow(cs).try_borrow_mut().ok()?;
        *slot = None;
        Some(HASH_OK)
    })
    .unwrap_or(HASH_FAILURE)
}
