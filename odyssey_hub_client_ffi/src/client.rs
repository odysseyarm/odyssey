use odyssey_hub_client::client::Client;
use std::ffi::c_void;

use crate::ffi_common::{Device, ScreenInfo};
use crate::funny::{Vector2f32, Vector3f32};
use crate::Handle;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserObj {
    pub ptr: *const c_void,
}

// cbindgen all of a sudden doesn't create opaque types
mod hack {
    #[allow(unused)]
    #[no_mangle]
    pub struct Client;
}

// SAFETY: Any user data object must be safe to send between threads.
unsafe impl Send for UserObj {}

#[repr(C)]
pub enum ClientError {
    None,
    NotConnected,
    StreamEnd,
    ConnectFailure,
}

#[no_mangle]
pub extern "C" fn client_new() -> *mut Client {
    Box::into_raw(Box::new(Client::default()))
}

#[no_mangle]
pub extern "C" fn client_free(client: *mut Client) {
    unsafe {
        drop(Box::from_raw(client));
    }
}

#[no_mangle]
pub extern "C" fn client_connect(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.connect().await {
            Ok(_) => callback(userdata, ClientError::None),
            Err(_) => callback(userdata, ClientError::ConnectFailure),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_get_device_list(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError, *mut Device, usize),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.get_device_list().await;
        match result {
            Ok(dl) => {
                let devices: Vec<Device> = dl.into_iter().map(Into::into).collect();
                let len = devices.len();
                let ptr = unsafe {
                    let layout = std::alloc::Layout::array::<Device>(len).unwrap();
                    let ptr = std::alloc::alloc(layout) as *mut Device;
                    ptr.copy_from_nonoverlapping(devices.as_ptr(), len);
                    ptr
                };
                callback(userdata, ClientError::None, ptr, len);
            }
            Err(_) => callback(userdata, ClientError::NotConnected, std::ptr::null_mut(), 0),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_start_stream(
    handle: *const crate::Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError, std::mem::MaybeUninit<crate::ffi_common::Event>),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.poll().await {
            Ok(mut stream) => loop {
                match stream.message().await {
                    Ok(Some(reply)) => {
                        let as_event: odyssey_hub_common::events::Event =
                            reply.event.unwrap().into();
                        callback(
                            userdata,
                            ClientError::None,
                            std::mem::MaybeUninit::new(as_event.into()),
                        );
                    }
                    Ok(None) => {
                        callback(
                            userdata,
                            ClientError::StreamEnd,
                            std::mem::MaybeUninit::uninit(),
                        );
                        break;
                    }
                    Err(_) => {
                        callback(
                            userdata,
                            ClientError::StreamEnd,
                            std::mem::MaybeUninit::uninit(),
                        );
                        break;
                    }
                }
            },
            Err(_) => {
                callback(
                    userdata,
                    ClientError::NotConnected,
                    std::mem::MaybeUninit::uninit(),
                );
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn client_stop_stream(handle: *const Handle, client: *mut Client) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        client.end_token.cancel();
    });
}

#[no_mangle]
pub extern "C" fn client_reset_zero(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: Device,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    let device_owned: odyssey_hub_common::device::Device = device.clone().into();

    tokio::spawn(async move {
        let result = client.reset_zero(device_owned).await;

        let error = if result.is_ok() {
            ClientError::None
        } else {
            ClientError::NotConnected
        };

        callback(userdata, error);
    });
}

#[no_mangle]
pub extern "C" fn client_zero(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: Device,
    translation: Vector3f32,
    target: Vector2f32,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    let translation = odyssey_hub_server_interface::Vector3 {
        x: translation.x,
        y: translation.y,
        z: translation.z,
    };
    let target = odyssey_hub_server_interface::Vector2 {
        x: target.x,
        y: target.y,
    };

    let device_owned: odyssey_hub_common::device::Device = device.clone().into();

    tokio::spawn(async move {
        let result = client.zero(device_owned, translation, target).await;
        let error = if result.is_ok() {
            ClientError::None
        } else {
            ClientError::NotConnected
        };
        callback(userdata, error);
    });
}

#[no_mangle]
pub extern "C" fn client_reset_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: Device,
    callback: extern "C" fn(UserObj, ClientError, u8),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    let device_owned: odyssey_hub_common::device::Device = device.clone().into();

    tokio::spawn(async move {
        let result = client.reset_shot_delay(device_owned).await;

        match result {
            Ok(delay) => callback(userdata, ClientError::None, delay),
            Err(_) => callback(userdata, ClientError::NotConnected, 0),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_get_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: Device,
    callback: extern "C" fn(UserObj, ClientError, u8),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    let device_owned: odyssey_hub_common::device::Device = device.clone().into();

    tokio::spawn(async move {
        let result = client.get_shot_delay(device_owned).await;

        match result {
            Ok(delay) => callback(userdata, ClientError::None, delay),
            Err(_) => callback(userdata, ClientError::NotConnected, 0),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_set_shot_delay(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: Device,
    delay_ms: u8,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    let device_owned: odyssey_hub_common::device::Device = device.clone().into();

    tokio::spawn(async move {
        let result = client.set_shot_delay(device_owned, delay_ms).await;

        let error = if result.is_ok() {
            ClientError::None
        } else {
            ClientError::NotConnected
        };

        callback(userdata, error);
    });
}

#[no_mangle]
pub extern "C" fn client_get_screen_info_by_id(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    screen_id: u8,
    callback: extern "C" fn(UserObj, ClientError, ScreenInfo),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.get_screen_info_by_id(screen_id).await;
        match result {
            Ok(info) => callback(userdata, ClientError::None, info.into()),
            Err(_) => {
                let dummy_screen_info = ScreenInfo {
                    id: 0,
                    tl: Vector2f32 { x: 0.0, y: 0.0 },
                    tr: Vector2f32 { x: 0.0, y: 0.0 },
                    bl: Vector2f32 { x: 0.0, y: 0.0 },
                    br: Vector2f32 { x: 0.0, y: 0.0 },
                };
                callback(userdata, ClientError::NotConnected, dummy_screen_info);
            }
        }
    });
}
