use repr_c_wrapper::*;

#[repr(C)]
pub struct Client(repr_c_wrapper_t!(odyssey_hub_client::client::Client));

#[repr(C)]
pub struct ClientErrorFuture(repr_c_wrapper_t!(async_ffi::FfiFuture<crate::client::ClientError>));

#[repr(C)]
pub struct DeviceListResultFuture(repr_c_wrapper_t!(async_ffi::FfiFuture<crate::client::DeviceListResult>));

impl From<async_ffi::FfiFuture<crate::client::ClientError>> for ClientErrorFuture {
    fn from(val: async_ffi::FfiFuture<crate::client::ClientError>) -> Self {
        Self(val.into())
    }
}

impl From<async_ffi::FfiFuture<crate::client::DeviceListResult>> for DeviceListResultFuture {
    fn from(val: async_ffi::FfiFuture<crate::client::DeviceListResult>) -> Self {
        Self(val.into())
    }
}
