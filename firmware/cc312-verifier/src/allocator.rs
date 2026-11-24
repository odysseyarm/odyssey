use core::cell::RefCell;
use core::ffi::c_void;
use core::ptr;

use critical_section::Mutex;

// shortcut: one 4 KiB SDK allocation, revise when SDK allocation requirements change.
#[repr(align(8))]
struct Arena {
    bytes: [u8; 4096],
    allocated: bool,
}

static ARENA: Mutex<RefCell<Arena>> = Mutex::new(RefCell::new(Arena {
    bytes: [0; 4096],
    allocated: false,
}));

#[unsafe(no_mangle)]
extern "C" fn mbedtls_calloc(count: usize, size: usize) -> *mut c_void {
    let Some(length) = count.checked_mul(size) else {
        return ptr::null_mut();
    };
    critical_section::with(|cs| {
        let mut arena = ARENA.borrow(cs).borrow_mut();
        if length == 0 || length > arena.bytes.len() || arena.allocated {
            return ptr::null_mut();
        }
        arena.bytes[..length].fill(0);
        arena.allocated = true;
        arena.bytes.as_mut_ptr().cast()
    })
}

#[unsafe(no_mangle)]
extern "C" fn mbedtls_free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    critical_section::with(|cs| {
        let mut arena = ARENA.borrow(cs).borrow_mut();
        assert!(
            arena.allocated && pointer == arena.bytes.as_mut_ptr().cast(),
            "SDK freed foreign allocation"
        );
        arena.allocated = false;
    });
}
