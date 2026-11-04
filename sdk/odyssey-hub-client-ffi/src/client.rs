use std::ffi::c_void;
use std::sync::Arc;

use futures_util::stream::StreamExt;
use odyssey_hub_client::client::Client as NativeClient;
use odyssey_hub_common as common;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::Handle;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserObj {
    pub ptr: *const c_void,
}

/// Opaque, thread-safe client handle. Calls borrow the handle only until they
/// return; asynchronous work owns its connection snapshot.
#[derive(Default)]
pub struct Client {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    client: Option<NativeClient>,
    end_token: CancellationToken,
}

impl Client {
    fn snapshot(&self) -> Option<NativeClient> {
        self.inner.lock().client.clone()
    }
}

// SAFETY: the C API requires callers to pass userdata that may be used from any thread.
unsafe impl Send for UserObj {}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum ClientError {
    None,
    NotConnected,
    StreamEnd,
    ConnectFailure,
    RpcFailure,
}

impl From<odyssey_hub_client::client::ClientError> for ClientError {
    fn from(_: odyssey_hub_client::client::ClientError) -> Self {
        Self::RpcFailure
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn client_new() -> *mut Client {
    Box::into_raw(Box::new(Client::default()))
}

/// # Safety
/// `client` must be null or an owned handle from `client_new`, not in use by another call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_free(client: *mut Client) {
    if !client.is_null() {
        unsafe {
            drop(Box::from_raw(client));
        }
    }
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_connect(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let state = unsafe { &*client }.inner.clone();

    tokio::spawn(async move {
        match NativeClient::connect().await {
            Ok(client) => {
                // Keep the cancellation token stored in the handle: stop/start
                // may have changed it while the connection was being opened.
                state.lock().client = Some(client);
                callback(userdata, ClientError::None);
            }
            Err(_) => callback(userdata, ClientError::ConnectFailure),
        }
    });
}

/// The device array is borrowed only for the duration of the callback.
/// Copy any devices needed afterward; do not free the array. An empty list is null.
/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_get_device_list(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError, *mut common::device::Device, usize),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, std::ptr::null_mut(), 0);
        };
        let result = client.get_device_list().await;
        match result {
            Ok(mut devices) => {
                let len = devices.len();
                let ptr = if devices.is_empty() {
                    std::ptr::null_mut()
                } else {
                    devices.as_mut_ptr()
                };
                callback(userdata, ClientError::None, ptr, len);
            }
            Err(error) => callback(userdata, error.into(), std::ptr::null_mut(), 0),
        }
    });
}

/// Starts an event subscription. A start after stop uses a fresh cancellation
/// token; resetting the handle's token before snapshotting keeps subsequent
/// stops attached to this subscription.
/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_start_stream(
    handle: *const crate::Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError, std::mem::MaybeUninit<crate::event::Event>),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let (client, end_token) = {
        let client = unsafe { &*client };
        let mut inner = client.inner.lock();
        if inner.end_token.is_cancelled() {
            inner.end_token = Default::default();
        }
        (inner.client.clone(), inner.end_token.clone())
    };

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(
                userdata,
                ClientError::NotConnected,
                std::mem::MaybeUninit::uninit(),
            );
        };
        match client.subscribe_events().await {
            Ok(stream) => {
                // Cancellation ends the stream, reported as `StreamEnd`.
                let mut stream = std::pin::pin!(stream.take_until(end_token.cancelled_owned()));
                loop {
                    match stream.next().await {
                        Some(Ok(event)) => {
                            let event = match crate::event::Event::try_from(event) {
                                Ok(event) => event,
                                Err(()) => continue,
                            };
                            callback(
                                userdata,
                                ClientError::None,
                                std::mem::MaybeUninit::new(event),
                            );
                        }
                        None => {
                            callback(
                                userdata,
                                ClientError::StreamEnd,
                                std::mem::MaybeUninit::uninit(),
                            );
                            break;
                        }
                        Some(Err(_)) => {
                            callback(
                                userdata,
                                ClientError::RpcFailure,
                                std::mem::MaybeUninit::uninit(),
                            );
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                callback(userdata, error.into(), std::mem::MaybeUninit::uninit());
            }
        }
    });
}

/// Requests cancellation before returning. The stream's terminal callback is
/// asynchronous; consumers must wait for it before freeing callback userdata.
/// # Safety
/// `client` must be a live handle. `_handle` is unused and may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_stop_stream(_handle: *const Handle, client: *mut Client) {
    let client = unsafe { &*client };
    client.inner.lock().end_token.cancel();
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_read_register(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    port: u8,
    bank: u8,
    address: u8,
    callback: extern "C" fn(UserObj, ClientError, u8),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, 0);
        };
        match client.device(device).read_register(port, bank, address).await {
            Ok(data) => callback(userdata, ClientError::None, data),
            Err(error) => callback(userdata, error.into(), 0),
        }
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_write_register(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    port: u8,
    bank: u8,
    address: u8,
    data: u8,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected);
        };
        let result = client
            .device(device)
            .write_register(port, bank, address, data)
            .await;
        let error = result
            .map(|_| ClientError::None)
            .unwrap_or_else(ClientError::from);
        callback(userdata, error);
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_flash_settings(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected);
        };
        let result = client.device(device).flash_settings().await;
        let error = result
            .map(|_| ClientError::None)
            .unwrap_or_else(ClientError::from);
        callback(userdata, error);
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_reset_zero(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected);
        };
        let result = client.device(device).reset_zero().await;

        let error = result
            .map(|_| ClientError::None)
            .unwrap_or_else(ClientError::from);

        callback(userdata, error);
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_zero(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    translation: crate::funny::Vector3f32,
    target: crate::funny::Vector2f32,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    let translation = odyssey_hub_server_interface::Vector3 {
        x: translation.x,
        y: translation.y,
        z: translation.z,
    };
    let target = odyssey_hub_server_interface::Vector2 {
        x: target.x,
        y: target.y,
    };

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected);
        };
        let result = client.device(device).zero(translation, target, None).await;
        let error = result
            .map(|_| ClientError::None)
            .unwrap_or_else(ClientError::from);
        callback(userdata, error);
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_reset_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError, u16),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, 0);
        };
        let result = client.device(device).reset_shot_delay().await;

        match result {
            Ok(delay) => callback(userdata, ClientError::None, delay),
            Err(error) => callback(userdata, error.into(), 0),
        }
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_get_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError, u16),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, 0);
        };
        let result = client.device(device).get_shot_delay().await;

        match result {
            Ok(delay) => callback(userdata, ClientError::None, delay),
            Err(error) => callback(userdata, error.into(), 0),
        }
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_set_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    delay_ms: u16,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected);
        };
        let result = client.device(device).set_shot_delay(delay_ms).await;

        let error = result
            .map(|_| ClientError::None)
            .unwrap_or_else(ClientError::from);

        callback(userdata, error);
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_get_screen_info_by_id(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    screen_id: u8,
    callback: extern "C" fn(UserObj, ClientError, crate::event::ScreenInfo),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let dummy = crate::event::ScreenInfo {
            id: 0,
            bounds: [crate::funny::Vector2f32 { x: 0.0, y: 0.0 }; 4],
        };
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, dummy);
        };
        let result = client.get_screen_info_by_id(screen_id).await;
        match result {
            Ok(info) => {
                let info: common::ScreenInfo = info.into();
                callback(userdata, ClientError::None, info.into());
            }
            Err(error) => {
                callback(userdata, error.into(), dummy);
            }
        }
    });
}

/// Each device array is borrowed only for the duration of its callback.
/// Copy any devices needed afterward; do not free the array. An empty list is null.
/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_subscribe_device_list(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError, *mut common::device::Device, usize),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, std::ptr::null_mut(), 0);
        };
        match client.subscribe_device_list().await {
            Ok(mut stream) => loop {
                match stream.next().await {
                    Some(Ok(mut devices)) => {
                        let len = devices.len();
                        let ptr = if devices.is_empty() {
                            std::ptr::null_mut()
                        } else {
                            devices.as_mut_ptr()
                        };
                        callback(userdata, ClientError::None, ptr, len);
                    }
                    None => {
                        callback(userdata, ClientError::StreamEnd, std::ptr::null_mut(), 0);
                        break;
                    }
                    Some(Err(_)) => {
                        callback(userdata, ClientError::RpcFailure, std::ptr::null_mut(), 0);
                        break;
                    }
                }
            },
            Err(error) => {
                callback(userdata, error.into(), std::ptr::null_mut(), 0);
            }
        }
    });
}

/// # Safety
/// The pointers and callback userdata must satisfy the crate's ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn client_subscribe_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError, u16),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &*client }.snapshot();

    tokio::spawn(async move {
        let Some(client) = client else {
            return callback(userdata, ClientError::NotConnected, 0);
        };
        match client.device(device).subscribe_shot_delay().await {
            Ok(mut stream) => loop {
                match stream.next().await {
                    Some(Ok(delay)) => {
                        callback(userdata, ClientError::None, delay);
                    }
                    None => {
                        callback(userdata, ClientError::StreamEnd, 0);
                        break;
                    }
                    Some(Err(_)) => {
                        callback(userdata, ClientError::RpcFailure, 0);
                        break;
                    }
                }
            },
            Err(error) => {
                callback(userdata, error.into(), 0);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_cancels_before_return_without_polling_the_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let handle = Handle { tokio_rt: runtime };
        let mut client = Client::default();
        let subscription_token = client.inner.lock().end_token.clone();
        unsafe { client_stop_stream(&handle, &mut client) };
        assert!(subscription_token.is_cancelled());
    }

    #[test]
    fn queued_register_read_owns_its_client_until_callback() {
        extern "C" fn complete(userdata: UserObj, error: ClientError, value: u8) {
            let sender = unsafe {
                Box::from_raw(userdata.ptr as *mut tokio::sync::oneshot::Sender<(ClientError, u8)>)
            };
            let _ = sender.send((error, value));
        }

        let handle = Handle {
            tokio_rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        };
        let client = client_new();
        let (sender, receiver) = tokio::sync::oneshot::channel::<(ClientError, u8)>();
        let userdata = UserObj {
            ptr: Box::into_raw(Box::new(sender)).cast(),
        };
        let device = common::device::Device {
            uuid: [1, 2, 3, 4, 5, 6],
            transport: common::device::Transport::Usb,
            capabilities: common::device::DeviceCapabilities::empty(),
            firmware_version: [0; 3],
            events_transport: common::device::EventsTransport::Wired,
            events_connected: false,
            product_id: 0,
            name: [0; 33],
            netcore_release: 0,
        };

        // The current-thread runtime has not polled the queued request yet.
        unsafe {
            client_read_register(&handle, client, userdata, device, 0, 0, 0, complete);
            client_free(client);
        }
        let reply = handle.tokio_rt.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(1), receiver)
                .await
                .expect("callback deadline")
                .expect("callback reply")
        });
        assert_eq!(reply, (ClientError::NotConnected, 0));
    }
}
