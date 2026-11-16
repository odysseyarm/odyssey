use embedded_storage_async::nor_flash::NorFlash;
use odyssey_protocol::marker_control::ENROLLMENT_SECONDS;
/// Since: Odyssey Desktop 0.7.4.
pub const BOND_PAGE_SIZE: u32 = 4096;
const RECORD_SIZE: usize = 92;
const COMMITTED: [u8; 4] = *b"MCB1";

/// Since: Odyssey Desktop 0.7.4.
#[expect(
    missing_debug_implementations,
    reason = "Debug output would expose persisted BLE keys"
)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OwnerBond {
    /// Since: Odyssey Desktop 0.7.4.
    pub address: [u8; 6],
    /// Since: Odyssey Desktop 0.7.4.
    pub ltk: [u8; 16],
    /// Since: Odyssey Desktop 0.7.4.
    pub irk: Option<[u8; 16]>,
    /// Since: Odyssey Desktop 0.7.4.
    pub security_level: u8,
}

impl OwnerBond {
    /// Since: Odyssey Desktop 0.7.4.
    pub fn permits(&self, encrypted: bool, current: Option<&Self>) -> bool {
        encrypted && current.is_some_and(|bond| bond == self)
    }
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Default)]
pub struct Enrollment {
    deadline: Option<u64>,
}

impl Enrollment {
    /// Since: Odyssey Desktop 0.7.4.
    pub const fn new() -> Self {
        Self { deadline: None }
    }
    /// Since: Odyssey Desktop 0.7.4.
    pub fn open(&mut self, now: u64) {
        self.deadline = Some(now.saturating_add(u64::from(ENROLLMENT_SECONDS) * 1000));
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn active(&self, now: u64) -> bool {
        self.deadline.is_some_and(|deadline| now < deadline)
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn remaining_seconds(&self, now: u64) -> u8 {
        let remaining = self.deadline.unwrap_or(now).saturating_sub(now);
        remaining.div_ceil(1000).min(u64::from(ENROLLMENT_SECONDS)) as u8
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn next_change_at(&self, now: u64) -> Option<u64> {
        let seconds = self.remaining_seconds(now);
        (seconds != 0).then(|| self.deadline.unwrap() - u64::from(seconds - 1) * 1000)
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn close(&mut self) {
        self.deadline = None;
    }
}

/// A bond persistence failure.
/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, PartialEq, Eq)]
pub enum StoreError<E> {
    /// Since: Odyssey Desktop 0.7.4.
    Flash(E),
    /// Since: Odyssey Desktop 0.7.4.
    /// Stored data failed integrity or format verification.
    Verification,
    /// Since: Odyssey Desktop 0.7.4.
    /// Enrollment expired before replacement bond was committed.
    Expired,
}

#[repr(align(4))]
struct Record([u8; RECORD_SIZE]);

fn encode(bond: OwnerBond, generation: u32) -> Record {
    let mut bytes = [0xff; RECORD_SIZE];
    bytes[4..8].copy_from_slice(&generation.to_le_bytes());
    bytes[8..14].copy_from_slice(&bond.address);
    bytes[14] = bond.security_level;
    bytes[15] = u8::from(bond.irk.is_some());
    bytes[16..32].copy_from_slice(&bond.ltk);
    bytes[32..48].copy_from_slice(&bond.irk.unwrap_or([0; 16]));
    for index in 4..48 {
        bytes[index + 44] = !bytes[index];
    }
    Record(bytes)
}

fn decode(bytes: &[u8; RECORD_SIZE]) -> Option<(OwnerBond, u32)> {
    if bytes[..4] != COMMITTED
        || bytes[15] > 1
        || (4..48).any(|index| bytes[index + 44] != !bytes[index])
        || !matches!(bytes[14], 1..=2)
    {
        return None;
    }
    Some((
        OwnerBond {
            address: bytes[8..14].try_into().ok()?,
            ltk: bytes[16..32].try_into().ok()?,
            irk: (bytes[15] == 1).then(|| bytes[32..48].try_into().unwrap()),
            security_level: bytes[14],
        },
        u32::from_le_bytes(bytes[4..8].try_into().ok()?),
    ))
}

/// Since: Odyssey Desktop 0.7.4.
#[expect(
    missing_debug_implementations,
    reason = "Debug formatting would expose flash and bond storage state"
)]
pub struct BondStore<F> {
    flash: F,
    start: u32,
    owner: Option<OwnerBond>,
    slot: usize,
    generation: u32,
}

impl<F: NorFlash> BondStore<F> {
    /// Since: Odyssey Desktop 0.7.4.
    /// Returns flash I/O failures from either bond page.
    pub async fn load(flash: F, start: u32) -> Result<Self, StoreError<F::Error>> {
        let mut store = Self {
            flash,
            start,
            owner: None,
            slot: 1,
            generation: 0,
        };
        for slot in 0..2 {
            let mut record = Record([0; RECORD_SIZE]);
            store
                .flash
                .read(start + slot as u32 * BOND_PAGE_SIZE, &mut record.0)
                .await
                .map_err(StoreError::Flash)?;
            if let Some((bond, generation)) = decode(&record.0)
                && (store.owner.is_none() || (generation.wrapping_sub(store.generation) as i32) > 0)
            {
                store.owner = Some(bond);
                store.slot = slot;
                store.generation = generation;
            }
        }
        Ok(store)
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn owner(&self) -> Option<OwnerBond> {
        self.owner
    }

    /// Since: Odyssey Desktop 0.7.4.
    /// Returns flash I/O failures, failed record verification, or expired enrollment.
    pub async fn persist(
        &mut self,
        bond: OwnerBond,
        allowed: impl Fn() -> bool,
    ) -> Result<(), StoreError<F::Error>> {
        if !matches!(bond.security_level, 1..=2) {
            return Err(StoreError::Verification);
        }
        let slot = 1 - self.slot;
        let address = self.start + slot as u32 * BOND_PAGE_SIZE;
        let generation = self.generation.wrapping_add(1);
        let mut record = encode(bond, generation);
        self.flash
            .erase(address, address + BOND_PAGE_SIZE)
            .await
            .map_err(StoreError::Flash)?;
        self.flash
            .write(address + 4, &record.0[4..])
            .await
            .map_err(StoreError::Flash)?;
        let mut verified = Record([0; RECORD_SIZE]);
        self.flash
            .read(address, &mut verified.0)
            .await
            .map_err(StoreError::Flash)?;
        if record.0 != verified.0 {
            return Err(StoreError::Verification);
        }
        if !allowed() {
            return Err(StoreError::Expired);
        }
        // Commit last: interrupted writes retain previous owner, never unwritten replacement.
        record.0[..4].copy_from_slice(&COMMITTED);
        self.flash
            .write(address, &record.0[..4])
            .await
            .map_err(StoreError::Flash)?;
        self.flash
            .read(address, &mut verified.0)
            .await
            .map_err(StoreError::Flash)?;
        if decode(&verified.0) != Some((bond, generation)) {
            return Err(StoreError::Verification);
        }
        self.owner = Some(bond);
        self.slot = slot;
        self.generation = generation;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    use embedded_storage_async::nor_flash::{ErrorType, ReadNorFlash};

    use super::*;

    #[derive(Clone)]
    struct Flash {
        bytes: [u8; 8192],
        fail_commit: bool,
    }
    impl ErrorType for Flash {
        type Error = embedded_storage_async::nor_flash::NorFlashErrorKind;
    }
    impl ReadNorFlash for Flash {
        const READ_SIZE: usize = 1;
        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            bytes.copy_from_slice(&self.bytes[offset as usize..offset as usize + bytes.len()]);
            Ok(())
        }
        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }
    impl NorFlash for Flash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;
        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            if self.fail_commit && bytes.len() == 4 {
                return Err(Self::Error::Other);
            }
            for (dest, value) in self.bytes[offset as usize..].iter_mut().zip(bytes) {
                *dest &= value;
            }
            Ok(())
        }
        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.bytes[from as usize..to as usize].fill(0xff);
            Ok(())
        }
    }
    fn run<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("memory flash unexpectedly pending"),
        }
    }
    fn bond(key: u8) -> OwnerBond {
        OwnerBond {
            address: [7; 6],
            ltk: [key; 16],
            irk: Some([9; 16]),
            security_level: 1,
        }
    }
    #[test]
    fn encryption_and_address_without_owner_key_never_authorize() {
        assert!(!bond(1).permits(false, Some(&bond(1))));
        assert!(!bond(1).permits(true, None));
        assert!(!bond(1).permits(true, Some(&bond(2))));
        assert!(bond(1).permits(true, Some(&bond(1))));
    }
    #[test]
    fn physical_window_reports_exact_countdown_boundaries() {
        let mut enrollment = Enrollment::default();
        assert_eq!(enrollment.remaining_seconds(0), 0);
        assert_eq!(enrollment.next_change_at(0), None);
        enrollment.open(100);
        for elapsed in 0..30 {
            let now = 100 + elapsed * 1000;
            assert_eq!(enrollment.remaining_seconds(now), (30 - elapsed) as u8);
            assert_eq!(
                enrollment.remaining_seconds(now + 999),
                (30 - elapsed) as u8
            );
            assert_eq!(enrollment.next_change_at(now), Some(now + 1000));
            assert!(enrollment.active(now + 999));
        }
        assert!(!enrollment.active(30_100));
        assert_eq!(enrollment.remaining_seconds(30_100), 0);
        assert_eq!(enrollment.next_change_at(30_100), None);
    }

    #[test]
    fn physical_window_reopens_closes_and_saturates() {
        let mut enrollment = Enrollment::new();
        enrollment.open(100);
        assert_eq!(enrollment.remaining_seconds(1100), 29);
        enrollment.open(1100);
        assert_eq!(enrollment.remaining_seconds(1100), 30);
        assert_eq!(enrollment.next_change_at(1100), Some(2100));
        enrollment.close();
        assert!(!enrollment.active(1100));
        assert_eq!(enrollment.remaining_seconds(1100), 0);
        assert_eq!(enrollment.next_change_at(1100), None);
        enrollment.open(2100);
        assert_eq!(enrollment.remaining_seconds(2100), 30);
        enrollment.open(u64::MAX - 1);
        assert_eq!(enrollment.remaining_seconds(u64::MAX - 1), 1);
        assert_eq!(enrollment.next_change_at(u64::MAX - 1), Some(u64::MAX));
        assert_eq!(enrollment.remaining_seconds(u64::MAX), 0);
    }
    #[test]
    fn persisted_owner_reloads_and_failed_replacement_keeps_old_owner() {
        let flash = Flash {
            bytes: [0xff; 8192],
            fail_commit: false,
        };
        let mut store = run(BondStore::load(flash, 0)).unwrap();
        run(store.persist(bond(1), || true)).unwrap();
        let mut store = run(BondStore::load(store.flash, 0)).unwrap();
        assert!(store.owner() == Some(bond(1)));
        store.flash.fail_commit = true;
        assert!(run(store.persist(bond(2), || true)).is_err());
        assert!(store.owner() == Some(bond(1)));
        let mut store = run(BondStore::load(store.flash, 0)).unwrap();
        assert!(store.owner() == Some(bond(1)));
        store.flash.fail_commit = false;
        assert_eq!(
            run(store.persist(bond(2), || false)),
            Err(StoreError::Expired)
        );
        assert!(run(BondStore::load(store.flash, 0)).unwrap().owner() == Some(bond(1)));
    }
    #[test]
    fn committed_reenrollment_revokes_old_key_after_reboot() {
        let flash = Flash {
            bytes: [0xff; 8192],
            fail_commit: false,
        };
        let mut store = run(BondStore::load(flash, 0)).unwrap();
        run(store.persist(bond(1), || true)).unwrap();
        run(store.persist(bond(2), || true)).unwrap();
        let store = run(BondStore::load(store.flash, 0)).unwrap();
        let owner = store.owner().unwrap();
        assert!(owner.permits(true, Some(&bond(2))));
        assert!(!owner.permits(true, Some(&bond(1))));
    }
    #[test]
    fn every_interrupted_record_prefix_rejects_new_owner() {
        let mut record = encode(bond(2), 2);
        for count in 0..RECORD_SIZE - 4 {
            let mut torn = [0xff; RECORD_SIZE];
            torn[4..4 + count].copy_from_slice(&record.0[4..4 + count]);
            assert!(decode(&torn).is_none());
        }
        record.0[..4].copy_from_slice(&COMMITTED);
        assert!(decode(&record.0).is_some());
        record.0[16] ^= 1;
        assert!(decode(&record.0).is_none());
    }
}
