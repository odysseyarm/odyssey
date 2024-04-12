use odyssey_hub_client::client::Client;

use crate::ffi_common::FfiDevice;

#[repr(C)]
pub enum ClientError {
    ClientErrorNone,
    ClientErrorConnectFailure,
    ClientErrorNotConnected,
    ClientErrorStreamEnd,
    ClientErrorEnd,
}

#[no_mangle]
pub extern "C" fn client_new() -> *mut Client {
    Box::into_raw(Box::new(Client::default()))
}

#[no_mangle]
pub extern "C" fn client_connect(client: *mut Client, callback: extern "C" fn (error: ClientError)) {
    let client = unsafe { &mut *client };
    tokio::spawn(async move {
        match client.connect().await {
            Ok(_) => callback(ClientError::ClientErrorNone),
            Err(_) => callback(ClientError::ClientErrorConnectFailure),
        }
    });
}

#[no_mangle]
pub extern "C" fn client_get_device_list(client: *mut Client, callback: extern "C" fn (error: ClientError, device_list: *mut crate::ffi_common::FfiDevice, size: usize)) {
    let client = unsafe { &mut *client };
    tokio::spawn(async move {
        match client.get_device_list().await {
            Ok(dl) => {
                // allocate memory for the device list, and copy the device list into it, and set len to the length of the device list
                let device_list = dl.into_iter().map(|d| d.into()).collect::<Vec<FfiDevice>>();
                unsafe {
                    let len = device_list.len();
                    let device_list_out = std::alloc::alloc(std::alloc::Layout::array::<FfiDevice>(device_list.len()).unwrap()) as *mut FfiDevice;
                    std::ptr::copy(device_list.as_ptr(), device_list_out, device_list.len());
                    callback(ClientError::ClientErrorNone, device_list_out, len);
                }
            },
            Err(_) => callback(ClientError::ClientErrorNotConnected, std::ptr::null_mut(), 0),
        }
    });
}

#[no_mangle]
pub extern "C" fn start_stream(client: *mut Client, callback: extern "C" fn (error: ClientError, reply: crate::ffi_common::FfiEvent)) {
    let client = unsafe { &mut *client };
    tokio::spawn(async move {
        match client.poll().await {
            Ok(mut stream) => {
                while let Some(reply) = stream.message().await.unwrap() {
                    let as_event: odyssey_hub_common::events::Event = reply.event.unwrap().into();
                    callback(ClientError::ClientErrorNone, as_event.into());
                }
                callback(ClientError::ClientErrorStreamEnd, odyssey_hub_common::events::Event::None.into());
            },
            Err(_) => { callback(ClientError::ClientErrorNotConnected, odyssey_hub_common::events::Event::None.into()); },
        }
    });
}
