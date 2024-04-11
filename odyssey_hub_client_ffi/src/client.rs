use async_ffi::{FfiFuture, FutureExt};
use odyssey_hub_client::client::Client;

use crate::ffi_common::FfiDevice;

#[repr(C)]
pub enum ClientError {
    ClientErrorNone,
    ClientErrorConnectFailure,
    ClientErrorNotConnected,
    ClientErrorErrorEnd,
}

#[no_mangle]
pub extern "C" fn client_new() -> *mut Client {
    Box::into_raw(Box::new(Client::default()))
}

#[no_mangle]
pub extern "C" fn client_connect(client: *mut Client) -> FfiFuture<ClientError> {
    let client = unsafe { &mut *client };
    async {
        match client.connect().await {
            Ok(_) => ClientError::ClientErrorNone,
            Err(_) => ClientError::ClientErrorConnectFailure,
        }
    }.into_ffi()
}

#[repr(C)]
pub struct DeviceListResult {
    error: ClientError,
    device_list: *mut crate::ffi_common::FfiDevice,
    len: usize,
}

#[no_mangle]
pub extern "C" fn client_get_device_list(client: *mut Client) -> FfiFuture<DeviceListResult> {
    let client = unsafe { &mut *client };
    async {
        match client.get_device_list().await {
            Ok(dl) => {
                // allocate memory for the device list, and copy the device list into it, and set len to the length of the device list
                let device_list = dl.into_iter().map(|d| d.into()).collect::<Vec<FfiDevice>>();
                unsafe {
                    let len = device_list.len();
                    let device_list_out = std::alloc::alloc(std::alloc::Layout::array::<FfiDevice>(device_list.len()).unwrap()) as *mut FfiDevice;
                    std::ptr::copy(device_list.as_ptr(), device_list_out, device_list.len());
                    DeviceListResult { error: ClientError::ClientErrorNone, device_list: device_list_out, len }
                }
            },
            Err(_) => DeviceListResult { error: ClientError::ClientErrorNotConnected, device_list: std::ptr::null_mut(), len: 0 },
        }
    }.into_ffi()
}

#[no_mangle]
pub extern "C" fn client_poll(client: *mut Client) -> FfiFuture<ClientError> {
    let client = unsafe { &mut *client };
    async {
        match client.poll().await {
            Ok(_) => ClientError::ClientErrorNone,
            Err(_) => ClientError::ClientErrorNotConnected,
        }
    }.into_ffi()
}
