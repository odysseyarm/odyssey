use interoptopus::{ffi_function, ffi_type};

#[ffi_type(skip(inner))]
#[derive(Default, Clone)]
pub struct Client {
    inner: odyssey_hub_client::client::Client,
}

#[ffi_type]
pub enum ClientError {
    ClientErrorNone,
    ClientErrorConnectFailure,
    ClientErrorNotConnected,
    ClientErrorStreamEnd,
    ClientErrorEnd,
}

#[ffi_type]
#[derive(Clone, Copy)]
pub struct UserObj(*const std::ffi::c_void);

// SAFETY: Any user data object must be safe to send between threads.
unsafe impl Send for UserObj {}

#[ffi_function]
pub extern "C" fn ohc_c_new() -> *mut Client {
    Box::into_raw(Box::new(Client::default()))
}

#[ffi_function]
pub extern "C" fn ohc_c_free(client: *mut Client) {
    unsafe {
        drop(Box::from_raw(client));
    };
}

#[ffi_function]
pub extern "C" fn ohc_c_connect(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    callback: extern "C" fn(userdata: UserObj, error: ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.inner.connect().await {
            Ok(_) => callback(userdata, ClientError::ClientErrorNone),
            Err(_) => callback(userdata, ClientError::ClientErrorConnectFailure),
        }
    });
}

#[ffi_function]
pub extern "C" fn ohc_c_get_device_list(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    callback: extern "C" fn(
        userdata: UserObj,
        error: ClientError,
        device_list: *mut crate::ffi_common::Device,
        size: usize,
    ),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let mut client = unsafe { &*client }.clone();

    tokio::spawn(async move {
        match client.inner.get_device_list().await {
            Ok(dl) => {
                // allocate memory for the device list, and copy the device list into it, and set len to the length of the device list
                let device_list = dl
                    .into_iter()
                    .map(|d| d.into())
                    .collect::<Vec<crate::ffi_common::Device>>();
                unsafe {
                    let len = device_list.len();
                    let device_list_out = std::alloc::alloc(
                        std::alloc::Layout::array::<crate::ffi_common::Device>(device_list.len())
                            .unwrap(),
                    ) as *mut crate::ffi_common::Device;
                    std::ptr::copy(device_list.as_ptr(), device_list_out, device_list.len());
                    callback(userdata, ClientError::ClientErrorNone, device_list_out, len);
                }
            }
            Err(_) => callback(
                userdata,
                ClientError::ClientErrorNotConnected,
                std::ptr::null_mut(),
                0,
            ),
        }
    });
}

#[ffi_function]
pub extern "C" fn ohc_c_start_stream(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    callback: extern "C" fn(
        userdata: UserObj,
        error: ClientError,
        error_msg: *const std::ffi::c_char,
        reply: crate::ffi_common::Event,
    ),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let mut client = unsafe { &*client }.clone();

    tokio::spawn(async move {
        match client.inner.poll().await {
            Ok(mut stream) => loop {
                match stream.message().await {
                    Ok(Some(reply)) => {
                        let as_event: odyssey_hub_common::events::Event =
                            reply.event.unwrap().into();
                        let err_msg = std::ffi::CString::new("").unwrap();
                        callback(
                            userdata,
                            ClientError::ClientErrorNone,
                            err_msg.as_ptr(),
                            as_event.into(),
                        );
                    }
                    Ok(None) => {
                        let err_msg = std::ffi::CString::new("Stream closed by sender").unwrap();
                        callback(
                            userdata,
                            ClientError::ClientErrorStreamEnd,
                            err_msg.as_ptr(),
                            odyssey_hub_common::events::Event::None.into(),
                        );
                        continue;
                    }
                    Err(val) => {
                        let err_msg = std::ffi::CString::new(val.message()).unwrap();
                        callback(
                            userdata,
                            ClientError::ClientErrorStreamEnd,
                            err_msg.as_ptr(),
                            odyssey_hub_common::events::Event::None.into(),
                        );
                        break;
                    }
                }
            },
            Err(_) => {
                let err_msg = std::ffi::CString::new("Client not connected").unwrap();
                callback(
                    userdata,
                    ClientError::ClientErrorNotConnected,
                    err_msg.as_ptr(),
                    odyssey_hub_common::events::Event::None.into(),
                );
            }
        }
    });
}

#[ffi_function]
pub extern "C" fn ohc_c_stop_stream(
    handle: *const crate::Handle,
    client: *mut Client,
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &*client };

    client.inner.end_token.cancel();
}

#[ffi_function]
pub extern "C" fn ohc_c_write_vendor(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    device: *const crate::ffi_common::Device,
    tag: u8,
    data: *const u8,
    len: usize,
    callback: extern "C" fn(userdata: UserObj, error: ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let mut client = unsafe { &*client }.clone();

    let device = unsafe { &*device }.clone().into();

    let data = unsafe { std::slice::from_raw_parts(data, len) }
        .iter()
        .map(|&x| x as u8)
        .collect::<Vec<u8>>();

    tokio::spawn(async move {
        match client.inner.write_vendor(device, tag, data).await {
            Ok(_) => callback(userdata, ClientError::ClientErrorNone),
            Err(_) => callback(userdata, ClientError::ClientErrorNotConnected),
        }
    });
}

#[ffi_function]
pub extern "C" fn ohc_c_reset_zero(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    device: *const crate::ffi_common::Device,
    callback: extern "C" fn(userdata: UserObj, error: ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let mut client = unsafe { &*client }.clone();

    let device = unsafe { &*device }.clone().into();

    tokio::spawn(async move {
        match client.inner.reset_zero(device).await {
            Ok(_) => callback(userdata, ClientError::ClientErrorNone),
            Err(_) => callback(userdata, ClientError::ClientErrorNotConnected),
        }
    });
}

#[ffi_function]
pub extern "C" fn ohc_c_zero(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    device: *const crate::ffi_common::Device,
    translation: *const crate::funny::Vector3f32,
    target: *const crate::funny::Vector2f32,
    callback: extern "C" fn(userdata: UserObj, error: ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let mut client = unsafe { &*client }.clone();

    let device = unsafe { &*device }.clone().into();

    let translation = unsafe { &*translation }.clone();
    let translation = odyssey_hub_server_interface::Vector3 {
        x: translation.x,
        y: translation.y,
        z: translation.z,
    };
    let target = unsafe { &*target }.clone();
    let target = odyssey_hub_server_interface::Vector2 {
        x: target.x,
        y: target.y,
    };

    tokio::spawn(async move {
        match client.inner.zero(device, translation, target).await {
            Ok(_) => callback(userdata, ClientError::ClientErrorNone),
            Err(_) => callback(userdata, ClientError::ClientErrorNotConnected),
        }
    });
}

#[ffi_function]
pub extern "C" fn ohc_c_get_screen_info_by_id(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    screen_id: u8,
    callback: extern "C" fn(
        userdata: UserObj,
        error: ClientError,
        error_msg: *const std::ffi::c_char,
        reply: crate::ffi_common::ScreenInfo,
    ),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let mut client = unsafe { &*client }.clone();

    tokio::spawn(async move {
        match client.inner.get_screen_info_by_id(screen_id).await {
            Ok(reply) => {
                let err_msg = std::ffi::CString::new("").unwrap();
                callback(
                    userdata,
                    ClientError::ClientErrorNone,
                    err_msg.as_ptr(),
                    reply.into(),
                );
            }
            Err(_) => {
                let err_msg = std::ffi::CString::new("Client not connected").unwrap();
                callback(
                    userdata,
                    ClientError::ClientErrorNotConnected,
                    err_msg.as_ptr(),
                    odyssey_hub_common::ScreenInfo::default().into(),
                );
            }
        }
    });
}
