use core::sync::atomic::{AtomicPtr, Ordering};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use protodongers::hub::HubMsg;
use static_cell::StaticCell;

pub static CONTROL_CH: StaticCell<Channel<ThreadModeRawMutex, HubMsg, 4>> = StaticCell::new();
static PTR: AtomicPtr<Channel<ThreadModeRawMutex, HubMsg, 4>> =
    AtomicPtr::new(core::ptr::null_mut());

pub fn init() -> &'static Channel<ThreadModeRawMutex, HubMsg, 4> {
    CONTROL_CH.init(Channel::new())
}

pub fn register(ch: &'static Channel<ThreadModeRawMutex, HubMsg, 4>) {
    PTR.store(ch as *const _ as *mut _, Ordering::Release);
}

#[allow(dead_code)]
pub fn try_send(evt: HubMsg) {
    let p = PTR.load(Ordering::Acquire);
    if !p.is_null() {
        // Safety: pointer set only to the static channel
        let ch: &Channel<ThreadModeRawMutex, HubMsg, 4> = unsafe { &*p };
        let _ = ch.try_send(evt);
    }
}
