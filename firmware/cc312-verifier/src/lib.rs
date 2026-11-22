#![no_std]
#![expect(unsafe_code, reason = "CC312 and Oberon cross the C ABI")]

#[cfg(not(feature = "shared-entry"))]
mod allocator;

#[cfg(all(feature = "shared-entry", feature = "export-entry"))]
compile_error!("shared-entry and export-entry cannot be enabled together");

#[cfg(not(feature = "shared-entry"))]
use core::cell::RefCell;
use core::fmt;
#[cfg(not(feature = "shared-entry"))]
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "export-entry")]
use core::sync::atomic::AtomicU8;

#[cfg(not(feature = "shared-entry"))]
use critical_section::Mutex;

#[cfg(not(feature = "shared-entry"))]
const HASH_OK: i32 = 0;
#[cfg(not(feature = "shared-entry"))]
const HASH_UNSUPPORTED: i32 = -134;
#[cfg(not(feature = "shared-entry"))]
const HASH_BAD_STATE: i32 = -137;
#[cfg(not(feature = "shared-entry"))]
const HASH_BUFFER_TOO_SMALL: i32 = -138;
#[cfg(not(feature = "shared-entry"))]
const HASH_FAILURE: i32 = -147;
#[cfg(not(feature = "shared-entry"))]
const SHA512_SIZE: usize = 64;

#[cfg(feature = "export-entry")]
#[unsafe(link_section = ".shared_verifier_bss")]
static mut VERIFY_LOCKED: AtomicBool = AtomicBool::new(false);
#[cfg(not(feature = "export-entry"))]
static VERIFY_LOCKED: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "export-entry")]
static SHARED_VERIFIER_INIT_STATE: AtomicU8 = AtomicU8::new(0);

fn verify_locked() -> &'static AtomicBool {
    #[cfg(feature = "export-entry")]
    // SAFETY: initialized before verifier use.
    return unsafe { &*core::ptr::addr_of!(VERIFY_LOCKED) };
    #[cfg(not(feature = "export-entry"))]
    &VERIFY_LOCKED
}
#[cfg(not(feature = "shared-entry"))]
#[cfg(feature = "export-entry")]
#[unsafe(link_section = ".shared_verifier_bss")]
static mut HASH_CONTEXT: Mutex<RefCell<Option<MaybeUninit<nrfxlib_sys::ocrypto_sha512_ctx>>>> =
    Mutex::new(RefCell::new(None));
#[cfg(all(not(feature = "shared-entry"), not(feature = "export-entry")))]
static HASH_CONTEXT: Mutex<RefCell<Option<MaybeUninit<nrfxlib_sys::ocrypto_sha512_ctx>>>> =
    Mutex::new(RefCell::new(None));

#[cfg(not(feature = "shared-entry"))]
fn hash_context() -> &'static Mutex<RefCell<Option<MaybeUninit<nrfxlib_sys::ocrypto_sha512_ctx>>>> {
    #[cfg(feature = "export-entry")]
    // SAFETY: initialized before verifier use; callbacks serialize context access.
    return unsafe { &*core::ptr::addr_of!(HASH_CONTEXT) };
    #[cfg(not(feature = "export-entry"))]
    &HASH_CONTEXT
}

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
        verify_locked()
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| Error::Busy)
    }
}

impl Drop for VerifyGuard {
    fn drop(&mut self) {
        verify_locked().store(false, Ordering::Release);
    }
}

/// Verify non-empty Ed25519 message through CryptoCell.
///
/// In `shared-entry` mode, this calls the stage1 function table at flash `0x7FF0`.
/// Caller must keep all buffers readable for the synchronous call. The exported
/// entry's `.shared_verifier_bss` must be initialized once by its owner before use;
/// later images must not clear or modify that state.
///
/// # Errors
///
/// Returns `EmptyMessage`, `Busy`, or the verifier's non-success status in `Vendor`.
pub fn verify(message: &[u8], key: &[u8; 32], sig: &[u8; 64]) -> Result<bool, Error> {
    if message.is_empty() {
        return Err(Error::EmptyMessage);
    }
    let _guard = VerifyGuard::acquire()?;
    #[cfg(feature = "shared-entry")]
    let status = unsafe { shared_entry_call(message.as_ptr(), message.len(), key.as_ptr(), sig.as_ptr()) };
    #[cfg(not(feature = "shared-entry"))]
    // SAFETY: inputs stay alive for synchronous C reads; C does not retain them.
    let status = unsafe {
        cc312_verify_ed25519(message.as_ptr(), message.len(), key.as_ptr(), sig.as_ptr())
    };
    match status {
        0 => Ok(true),
        -149 => Ok(false),
        #[cfg(not(feature = "shared-entry"))]
        -135 => Ok(false),
        code => Err(Error::Vendor(code)),
    }
}

fn invalid_abi_parameters(
    message: *const u8,
    message_length: usize,
    key: *const u8,
    signature: *const u8,
) -> bool {
    message_length == 0 || message.is_null() || key.is_null() || signature.is_null()
}

#[cfg(feature = "shared-entry")]
type VerifyFn = unsafe extern "C" fn(*const u8, usize, *const u8, *const u8) -> i32;

#[cfg(feature = "shared-entry")]
#[repr(C)]
struct SharedVerifyEntry {
    magic: u32,
    abi_version: u32,
    verify: Option<VerifyFn>,
    reserved: u32,
}
#[cfg(all(feature = "shared-entry", target_pointer_width = "32"))]
const _: () = assert!(core::mem::size_of::<SharedVerifyEntry>() == 16);

#[cfg(feature = "shared-entry")]
const SHARED_ENTRY_ADDRESS: usize = 0x7FF0;
#[cfg(feature = "shared-entry")]
const SHARED_ENTRY_MAGIC: u32 = 0x4F44_5956;

#[cfg(feature = "shared-entry")]
unsafe fn shared_entry_call(
    message: *const u8,
    message_length: usize,
    key: *const u8,
    signature: *const u8,
) -> i32 {
    // SAFETY: caller targets platform-mapped flash at the fixed ABI address.
    let entry = unsafe { &*(SHARED_ENTRY_ADDRESS as *const SharedVerifyEntry) };
    if entry.magic != SHARED_ENTRY_MAGIC || entry.abi_version != 1 || entry.reserved != 0 {
        return -137;
    }
    let Some(verify) = entry.verify else {
        return -137;
    };
    // SAFETY: safe API provides valid, synchronous borrowed buffers.
    unsafe { verify(message, message_length, key, signature) }
}

#[cfg(feature = "export-entry")]
/// Initialize verifier-owned RAM state once, before any verification.
///
/// Stage1 must own and initialize `.shared_verifier_bss`; later images must leave
/// the entire section untouched, including arena, lock, and hash callback state.
pub fn initialize_shared_verifier() {
    if SHARED_VERIFIER_INIT_STATE
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    unsafe {
        core::ptr::write(
            core::ptr::addr_of_mut!(HASH_CONTEXT),
            Mutex::new(RefCell::new(None)),
        );
        core::ptr::write(
            core::ptr::addr_of_mut!(VERIFY_LOCKED),
            AtomicBool::new(false),
        );
    }
    allocator::initialize();
    SHARED_VERIFIER_INIT_STATE.store(2, Ordering::Release);
}

#[cfg(not(feature = "shared-entry"))]
// SAFETY: C shim accepts borrowed buffers only for each synchronous call.
unsafe extern "C" {
    fn cc312_verify_ed25519(
        message: *const u8,
        message_length: usize,
        key: *const u8,
        signature: *const u8,
    ) -> i32;
}

#[cfg(feature = "export-entry")]
#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_shared_verify(
    message: *const u8,
    message_length: usize,
    key: *const u8,
    signature: *const u8,
) -> i32 {
    if invalid_abi_parameters(message, message_length, key, signature) {
        return -137;
    }
    let _guard = match VerifyGuard::acquire() {
        Ok(guard) => guard,
        Err(Error::Busy) => return -137,
        Err(Error::EmptyMessage) => return -137,
        Err(Error::Vendor(code)) => return code,
    };
    // SAFETY: ABI caller guarantees all non-null buffers span requested sizes and remain
    // readable during this synchronous call; arbitrary-pointer fault safety is not promised.
    unsafe { cc312_verify_ed25519(message, message_length, key, signature) }
}

#[cfg(feature = "export-entry")]
#[repr(C)]
struct ExportVerifyEntry {
    magic: u32,
    abi_version: u32,
    verify: Option<unsafe extern "C" fn(*const u8, usize, *const u8, *const u8) -> i32>,
    reserved: u32,
}

#[cfg(feature = "export-entry")]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".shared_verify_entry")]
static cc312_shared_verify_entry: ExportVerifyEntry = ExportVerifyEntry {
    magic: 0x4F44_5956,
    abi_version: 1,
    verify: Some(cc312_shared_verify),
    reserved: 0,
};

#[cfg(all(feature = "export-entry", target_pointer_width = "32"))]
const _: () = assert!(core::mem::size_of::<ExportVerifyEntry>() == 16);

#[cfg(not(feature = "shared-entry"))]
#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_setup(supported: bool) -> i32 {
    if !supported {
        return HASH_UNSUPPORTED;
    }
    critical_section::with(|cs| {
        let mut slot = hash_context().borrow(cs).try_borrow_mut().ok()?;
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

#[cfg(not(feature = "shared-entry"))]
#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_update(input: *const u8, input_length: usize) -> i32 {
    if input_length != 0 && input.is_null() {
        return HASH_BAD_STATE;
    }
    critical_section::with(|cs| {
        let mut slot = hash_context().borrow(cs).try_borrow_mut().ok()?;
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

#[cfg(not(feature = "shared-entry"))]
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
        let mut slot = hash_context().borrow(cs).try_borrow_mut().ok()?;
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

#[cfg(not(feature = "shared-entry"))]
#[unsafe(no_mangle)]
unsafe extern "C" fn cc312_oberon_hash_abort() -> i32 {
    critical_section::with(|cs| {
        let mut slot = hash_context().borrow(cs).try_borrow_mut().ok()?;
        *slot = None;
        Some(HASH_OK)
    })
    .unwrap_or(HASH_FAILURE)
}

#[cfg(test)]
mod tests {
    use super::invalid_abi_parameters;

    #[test]
    fn abi_rejects_empty_and_null_inputs() {
        let valid = 1usize as *const u8;
        assert!(invalid_abi_parameters(valid, 0, valid, valid));
        assert!(invalid_abi_parameters(core::ptr::null(), 1, valid, valid));
        assert!(invalid_abi_parameters(valid, 1, core::ptr::null(), valid));
        assert!(invalid_abi_parameters(valid, 1, valid, core::ptr::null()));
        assert!(!invalid_abi_parameters(valid, 1, valid, valid));
    }
}
