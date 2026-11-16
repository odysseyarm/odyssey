#![expect(
    unsafe_code,
    reason = "C ABI pointer handling requires unsafe operations"
)]
//! C ABI ownership contract:
//!
//! Unless explicitly documented as nullable, pointers must be non-null, aligned,
//! and valid for the duration of the call. Free functions accept null and consume
//! non-null handles exactly once. Do not free a handle concurrently with a call
//! borrowing it. Client calls may otherwise run concurrently.
//!
//! Asynchronous operations retain their own client state, so the client handle
//! may be freed after scheduling them. Callbacks run on runtime worker threads.
//! Userdata must be thread-safe and remain valid until the operation's final
//! callback. Keep the runtime alive until all required callbacks have completed.
//! Freeing it cancels pending work, so terminal callbacks can be lost.
//! Already-running callbacks may finish after `rt_free` returns.
//! Event callback payloads are initialized only when the error is `None`.
//! `NotConnected` means no connection has been established. `RpcFailure` means
//! a request or stream failed. `StreamEnd` means a stream closed without an error.
//! A client's connection runs on the runtime passed to `client_connect`. After
//! that runtime is freed, calls on the client fail with `RpcFailure` until it
//! connects again on a live runtime.
//! The C event ABI omits event kinds it cannot represent.
//! Headers are generated with the `odyssey-hub-common` dependency parsed, so
//! shared device layouts come from their Rust definitions, not from copies.
//!
/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub mod client;
/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub mod event;
/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub mod funny;
/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub mod packet;
/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub mod tracking_history;

/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub struct Handle {
    tokio_rt: tokio::runtime::Runtime,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle").finish_non_exhaustive()
    }
}

/// Creates a runtime, returning null if worker threads cannot be started.
///
/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub extern "C" fn rt_init() -> *mut Handle {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .ok();
    let Some(tokio_rt) = tokio_rt else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(Handle { tokio_rt }))
}

/// # Safety
/// `handle` must be null or an owned runtime from `rt_init`, not borrowed by
/// another call. Pending work is cancelled. See the crate's ownership contract.
///
/// # Since
/// FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_free(handle: *mut Handle) {
    if !handle.is_null() {
        // SAFETY: The function requires an owned, unborrowed handle from `rt_init`.
        let handle = unsafe { Box::from_raw(handle) };
        if tokio::runtime::Handle::try_current().is_ok() {
            // Blocking Runtime::drop panics in async contexts, including a
            // callback on this runtime's own worker thread.
            handle.tokio_rt.shutdown_background();
        } else {
            // Preserve synchronous shutdown for ordinary foreign callers.
            drop(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_can_be_released_from_its_worker_thread() {
        fn release(userdata: client::UserObj) {
            // SAFETY: `userdata.ptr` stores the owned handle. The callback runs once.
            unsafe { rt_free(userdata.ptr as *mut Handle) };
        }

        let handle = Box::into_raw(Box::new(Handle {
            tokio_rt: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("test runtime"),
        }));
        let userdata = client::UserObj { ptr: handle.cast() };
        let (start, started) = tokio::sync::oneshot::channel();
        let (done, completed) = std::sync::mpsc::channel();
        // SAFETY: The test retains the owned runtime until this spawn returns.
        unsafe { &*handle }.tokio_rt.spawn(async move {
            if started.await.is_ok() {
                release(userdata);
                let _ = done.send(());
            }
        });
        // Do not release the handle until the synchronous spawn borrow ends.
        start.send(()).expect("start shutdown");
        completed
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("worker-thread shutdown completed");
    }
}
