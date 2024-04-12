use std::mem::ManuallyDrop;

use async_ffi::FfiFuture;

use crate::client::{ClientError, DeviceListResult};

#[repr(C)]
pub struct ReprCWrapper<T>
    where [(); (std::mem::size_of::<ManuallyDrop::<T>>() + std::mem::size_of::<u64>() - 1) / std::mem::size_of::<u64>()]:
{
    bytes: [u64; (std::mem::size_of::<ManuallyDrop::<T>>() + std::mem::size_of::<u64>() - 1) / std::mem::size_of::<u64>()],
}

pub struct Client(ReprCWrapper<odyssey_hub_client::client::Client>);

impl From<FfiFuture<ClientError>> for ReprCWrapper<FfiFuture<ClientError>> {
    fn from(val: FfiFuture<ClientError>) -> Self {
        unsafe {
            let mut bytes = [0; (std::mem::size_of::<ManuallyDrop<FfiFuture<ClientError>>>() + std::mem::size_of::<u64>() - 1) / std::mem::size_of::<u64>()];
            std::ptr::copy(&val, bytes.as_mut_ptr() as *mut FfiFuture<ClientError>, 1);
            std::mem::forget(val);
            Self { bytes }
        }
    }
}

impl From<FfiFuture<DeviceListResult>> for ReprCWrapper<FfiFuture<DeviceListResult>> {
    fn from(val: FfiFuture<DeviceListResult>) -> Self {
        unsafe {
            let mut bytes = [0; (std::mem::size_of::<ManuallyDrop<FfiFuture<DeviceListResult>>>() + std::mem::size_of::<u64>() - 1) / std::mem::size_of::<u64>()];
            std::ptr::copy(&val, bytes.as_mut_ptr() as *mut FfiFuture<DeviceListResult>, 1);
            std::mem::forget(val);
            Self { bytes }
        }
    }
}
