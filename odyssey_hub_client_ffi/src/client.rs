use futures_util::stream::StreamExt;
use odyssey_hub_client::client::Client;
use std::ffi::c_void;

use crate::funny::{Vector2f32, Vector3f32};
use crate::Handle;

use odyssey_hub_common as common;

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
    callback: extern "C" fn(UserObj, ClientError, *mut common::device::Device, usize),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.get_device_list().await;
        match result {
            Ok(dl) => {
                let devices: Vec<common::device::Device> = dl.into_iter().map(Into::into).collect();
                let len = devices.len();
                let ptr = unsafe {
                    let layout = std::alloc::Layout::array::<common::device::Device>(len).unwrap();
                    let ptr = std::alloc::alloc(layout) as *mut common::device::Device;
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
    callback: extern "C" fn(UserObj, ClientError, std::mem::MaybeUninit<common::events::Event>),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.subscribe_events().await {
            Ok(mut stream) => loop {
                match stream.next().await {
                    Some(Ok(event)) => {
                        let event: odyssey_hub_common::events::Event = event.into();
                        callback(
                            userdata,
                            ClientError::None,
                            std::mem::MaybeUninit::new(event.into()),
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
pub extern "C" fn client_read_register(
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
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.read_register(device, port, bank, address).await {
            Ok(data) => callback(userdata, ClientError::None, data),
            Err(_) => callback(userdata, ClientError::NotConnected, 0),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_write_register(
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
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client
            .write_register(device, port, bank, address, data)
            .await;
        let error = if result.is_ok() {
            ClientError::None
        } else {
            ClientError::NotConnected
        };
        callback(userdata, error);
    });
}

#[no_mangle]
pub extern "C" fn client_flash_settings(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.flash_settings(device).await;
        let error = if result.is_ok() {
            ClientError::None
        } else {
            ClientError::NotConnected
        };
        callback(userdata, error);
    });
}

#[no_mangle]
pub extern "C" fn client_reset_zero(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.reset_zero(device).await;

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
    device: common::device::Device,
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

    tokio::spawn(async move {
        let result = client.zero(device, translation, target).await;
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
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError, u16),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.reset_shot_delay(device).await;

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
    device: common::device::Device,
    callback: extern "C" fn(UserObj, ClientError, u16),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.get_shot_delay(device).await;

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
    device: common::device::Device,
    delay_ms: u16,
    callback: extern "C" fn(UserObj, ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.set_shot_delay(device, delay_ms).await;

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
    callback: extern "C" fn(UserObj, ClientError, common::ScreenInfo),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        let result = client.get_screen_info_by_id(screen_id).await;
        match result {
            Ok(info) => callback(userdata, ClientError::None, info.into()),
            Err(_) => {
                let dummy_screen_info = common::ScreenInfo {
                    id: 0,
                    bounds: [nalgebra::Vector2::zeros(); 4],
                };
                callback(userdata, ClientError::NotConnected, dummy_screen_info);
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn client_subscribe_device_list(
    handle: *const Handle,
    client: *mut Client,
    userdata: UserObj,
    callback: extern "C" fn(UserObj, ClientError, *mut common::device::Device, usize),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();
    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.subscribe_device_list().await {
            Ok(mut stream) => loop {
                match stream.next().await {
                    Some(Ok(device_list)) => {
                        let devices: Vec<common::device::Device> =
                            device_list.into_iter().map(Into::into).collect();
                        let len = devices.len();
                        let ptr = if len > 0 {
                            unsafe {
                                let layout =
                                    std::alloc::Layout::array::<common::device::Device>(len)
                                        .unwrap();
                                let ptr = std::alloc::alloc(layout) as *mut common::device::Device;
                                ptr.copy_from_nonoverlapping(devices.as_ptr(), len);
                                ptr
                            }
                        } else {
                            std::ptr::null_mut()
                        };
                        callback(userdata, ClientError::None, ptr, len);
                    }
                    None => {
                        callback(userdata, ClientError::StreamEnd, std::ptr::null_mut(), 0);
                        break;
                    }
                    Some(Err(_)) => {
                        callback(userdata, ClientError::StreamEnd, std::ptr::null_mut(), 0);
                        break;
                    }
                }
            },
            Err(_) => {
                callback(userdata, ClientError::NotConnected, std::ptr::null_mut(), 0);
            }
        }
    });
}
