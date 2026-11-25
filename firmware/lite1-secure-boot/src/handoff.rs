//! Boot-accepted release keys; later images do not re-walk root history.
#![cfg_attr(
    all(feature = "hardware", target_arch = "arm"),
    expect(unsafe_code, reason = "stage1-owned RAM shared across firmware images")
)]

use release_signing::{KEY_SLOTS, PublicKey, TrustList};

use crate::Error;

const MAGIC: u32 = 0x3250_594f;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct BootPolicy {
    magic: u32,
    generation: u16,
    key_count: u8,
    reserved: u8,
    pending_address: u32,
    keys: [PublicKey; KEY_SLOTS],
    root_keys: [PublicKey; KEY_SLOTS],
}

const _: () = assert!(core::mem::size_of::<BootPolicy>() <= 0x200);
impl BootPolicy {
    pub fn keys(&self) -> &[PublicKey] {
        &self.keys[..usize::from(self.key_count)]
    }

    pub const fn root_keys(&self) -> &[PublicKey; KEY_SLOTS] {
        &self.root_keys
    }
    pub const fn generation(&self) -> u16 {
        self.generation
    }

    fn new(policy: &TrustList, pending_address: u32) -> Self {
        let mut keys = [[0; 32]; KEY_SLOTS];
        keys[..policy.keys().len()].copy_from_slice(policy.keys());
        Self {
            magic: MAGIC,
            generation: policy.generation(),
            key_count: policy.keys().len() as u8,
            reserved: 0,
            pending_address,
            keys,
            root_keys: *policy.root_set().roots(),
        }
    }

    fn validate(self) -> Result<Self, Error> {
        if self.magic != MAGIC
            || self.reserved != 0
            || self.generation == 0
            || !(1..=KEY_SLOTS).contains(&usize::from(self.key_count))
            || self.pending_address == 0
            || self.pending_address & 0xfff != 0
        {
            return Err(Error::Storage);
        }
        Ok(self)
    }
}

#[cfg(all(feature = "hardware", target_arch = "arm"))]
unsafe extern "C" {
    static __policy_handoff_start: BootPolicy;
}

#[cfg(all(feature = "hardware", target_arch = "arm"))]
pub fn current_policy() -> Result<BootPolicy, Error> {
    // SAFETY: linker reserves this aligned RAM block; stage1 writes it before handoff.
    let policy = unsafe { core::ptr::read_volatile(&raw const __policy_handoff_start) };
    policy.validate()
}

#[cfg(all(feature = "hardware", target_arch = "arm"))]
pub fn pending_record_address() -> Result<u32, Error> {
    Ok(current_policy()?.pending_address)
}

/// Called only by stage1 after verifying the policy and before transferring control.
#[cfg(all(feature = "hardware", target_arch = "arm"))]
pub fn publish(policy: &TrustList, pending_address: u32) {
    let handoff = BootPolicy::new(policy, pending_address);
    // SAFETY: stage1 exclusively owns the linker-reserved block until it transfers control.
    unsafe { core::ptr::write_volatile((&raw const __policy_handoff_start).cast_mut(), handoff) };
}

#[cfg(test)]
mod tests {
    use release_signing::RootSet;

    use super::*;

    #[test]
    fn handoff_preserves_active_keys_and_rejects_invalid_bounds() {
        let roots = RootSet::new(1, [[1; 32], [0; 32], [0; 32], [0; 32]]).unwrap();
        let list = TrustList::new(2, &[[2; 32], [3; 32]], roots).unwrap();
        let valid = BootPolicy::new(&list, 0xc8000).validate().unwrap();
        assert_eq!(valid.keys(), list.keys());
        assert_eq!(valid.generation(), 2);
        for count in [0, 5, u8::MAX] {
            assert!(
                BootPolicy {
                    key_count: count,
                    ..valid
                }
                .validate()
                .is_err()
            );
        }
        assert!(BootPolicy { magic: 0, ..valid }.validate().is_err());
        assert!(
            BootPolicy {
                pending_address: 0xc8001,
                ..valid
            }
            .validate()
            .is_err()
        );
    }
}
