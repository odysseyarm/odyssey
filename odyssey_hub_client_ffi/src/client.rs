use odyssey_hub_client::client::Client;

#[repr(C)]
pub enum ClientError {
    ClientErrorNone,
    ClientErrorConnectFailure,
    ClientErrorNotConnected,
    ClientErrorStreamEnd,
    ClientErrorEnd,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserObj(*const std::ffi::c_void);

// SAFETY: Any user data object must be safe to send between threads.
unsafe impl Send for UserObj {}

#[no_mangle]
pub extern "C" fn client_new() -> Box<Client> {
    Box::new(Client::default())
}

#[no_mangle]
pub extern "C" fn client_connect(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    callback: extern "C" fn(userdata: UserObj, error: ClientError),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &mut *client };

    tokio::spawn(async move {
        match client.connect().await {
            Ok(_) => callback(userdata, ClientError::ClientErrorNone),
            Err(_) => callback(userdata, ClientError::ClientErrorConnectFailure),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_get_device_list(
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

    let client = unsafe { &mut *client };
    tokio::spawn(async move {
        match client.get_device_list().await {
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

#[no_mangle]
pub extern "C" fn start_stream(
    handle: *const crate::Handle,
    userdata: UserObj,
    client: *mut Client,
    callback: extern "C" fn(userdata: UserObj, error: ClientError, reply: crate::ffi_common::Event),
) {
    let handle = unsafe { &*handle };
    let _guard = handle.tokio_rt.enter();

    let client = unsafe { &mut *client };
    tokio::spawn(async move {
        match client.poll().await {
            Ok(mut stream) => {
                while let Some(reply) = stream.message().await.unwrap() {
                    let as_event: odyssey_hub_common::events::Event = reply.event.unwrap().into();
                    callback(userdata, ClientError::ClientErrorNone, as_event.into());
                }
                callback(
                    userdata,
                    ClientError::ClientErrorStreamEnd,
                    odyssey_hub_common::events::Event::None.into(),
                );
            }
            Err(_) => {
                callback(
                    userdata,
                    ClientError::ClientErrorNotConnected,
                    odyssey_hub_common::events::Event::None.into(),
                );
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn write_vendor(
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

    let client = unsafe { &mut *client };
    let device = unsafe { &*device }.clone().into();

    let data = unsafe { std::slice::from_raw_parts(data, len) }
        .iter()
        .map(|&x| x as u8)
        .collect::<Vec<u8>>();

    tokio::spawn(async move {
        match client.write_vendor(device, tag, data).await {
            Ok(_) => callback(userdata, ClientError::ClientErrorNone),
            Err(_) => callback(userdata, ClientError::ClientErrorNotConnected),
        }
    });
}
