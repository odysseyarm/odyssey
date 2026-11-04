//! Which address each device connection task serves, and how to stop it.
//!
//! The executor's task pool and this table both hold `MAX_DEVICES` entries.
//! [`BleManager`](super::BleManager) claims a slot before it spawns a task and
//! the task releases it as its last action, so an address that still occupies
//! a slot still has a live task, even after it left the scan targets.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use odyssey_protocol::mux::MAX_DEVICES;

use super::central::Uuid;

/// A claimed slot, owned by exactly one device task.
#[derive(Clone, Copy)]
pub struct TaskSlot(usize);

pub enum ClaimError {
    /// A task for this address is still alive, possibly finishing a stop.
    Running,
    /// Every slot is held by another address.
    Full,
}

static SLOTS: BlockingMutex<CriticalSectionRawMutex, RefCell<[Option<Uuid>; MAX_DEVICES]>> =
    BlockingMutex::new(RefCell::new([None; MAX_DEVICES]));
static STOP: [Signal<CriticalSectionRawMutex, ()>; MAX_DEVICES] =
    [const { Signal::new() }; MAX_DEVICES];
static RELEASED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Reserves a slot for a new task serving `addr` and clears any stop request
/// left in it.
///
/// # Errors
///
/// [`ClaimError::Running`] while an earlier task for `addr` has not released
/// its slot, [`ClaimError::Full`] when no slot is free.
pub fn claim(addr: Uuid) -> Result<TaskSlot, ClaimError> {
    SLOTS.lock(|slots| {
        let mut slots = slots.borrow_mut();
        if slots.contains(&Some(addr)) {
            return Err(ClaimError::Running);
        }
        let index = slots
            .iter()
            .position(Option::is_none)
            .ok_or(ClaimError::Full)?;
        slots[index] = Some(addr);
        STOP[index].reset();
        Ok(TaskSlot(index))
    })
}

/// Frees `slot`. The task that owns it calls this as its last action; the
/// manager calls it only when spawning failed.
pub fn release(slot: TaskSlot) {
    SLOTS.lock(|slots| slots.borrow_mut()[slot.0] = None);
    RELEASED.signal(());
}

/// Resolves once the task in `slot` has been asked to stop. The task selects
/// this against all of its work.
pub async fn stop_requested(slot: TaskSlot) {
    STOP[slot.0].wait().await
}

/// Asks every task whose address is not in `targets` to stop. Does not wait.
pub fn stop_untargeted(targets: &[Uuid]) {
    SLOTS.lock(|slots| {
        for (index, addr) in slots.borrow().iter().enumerate() {
            if addr.is_some_and(|addr| !targets.contains(&addr)) {
                STOP[index].signal(());
            }
        }
    });
}

/// Asks every task to stop and waits until all of them have released their
/// slots, which happens after they have disconnected and freed their device
/// queue. Only one caller may wait at a time.
pub async fn stop_all() {
    loop {
        let any_live = SLOTS.lock(|slots| {
            let mut any_live = false;
            for (index, addr) in slots.borrow().iter().enumerate() {
                if addr.is_some() {
                    STOP[index].signal(());
                    any_live = true;
                }
            }
            any_live
        });
        if !any_live {
            return;
        }
        RELEASED.wait().await;
    }
}
