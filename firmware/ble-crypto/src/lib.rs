#![no_std]

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_crypto::driver::Rng;
use embassy_crypto_rustcrypto as _;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};

static RNG: Mutex<RefCell<Option<ChaCha20Rng>>> = Mutex::new(RefCell::new(None));

/// Seeds BLE cryptography once per boot with 32 bytes from the hardware RNG.
/// Panics if initialized twice; no random output is available before initialization.
pub fn init(seed: [u8; 32]) {
    critical_section::with(|cs| {
        let mut rng = RNG.borrow(cs).borrow_mut();
        assert!(rng.is_none(), "BLE crypto initialized twice");
        *rng = Some(ChaCha20Rng::from_seed(seed));
    });
}

struct Driver;

impl Rng for Driver {
    fn fill_bytes(buf: &mut [u8]) {
        critical_section::with(|cs| {
            RNG.borrow(cs)
                .borrow_mut()
                .as_mut()
                .expect("BLE crypto must be hardware-seeded before use")
                .fill_bytes(buf);
        });
    }
}

embassy_crypto::rng_impl!(Driver);

#[cfg(test)]
mod tests {
    #[test]
    fn registered_rng_preserves_chacha20_seed_stream() {
        super::init([0; 32]);
        let mut bytes = [0; 32];
        embassy_crypto::rng_fill_bytes(&mut bytes[..16]);
        embassy_crypto::rng_fill_bytes(&mut bytes[16..]);
        assert_eq!(
            bytes,
            [
                0x76, 0xb8, 0xe0, 0xad, 0xa0, 0xf1, 0x3d, 0x90, 0x40, 0x5d, 0x6a, 0xe5, 0x53, 0x86,
                0xbd, 0x28, 0xbd, 0xd2, 0x19, 0xb8, 0xa0, 0x8d, 0xed, 0x1a, 0xa8, 0x36, 0xef, 0xcc,
                0x8b, 0x77, 0x0d, 0xc7,
            ]
        );
    }
}
