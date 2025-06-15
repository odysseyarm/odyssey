use std::sync::Arc;

pub mod client;
pub mod ffi_common;
pub mod funny;

#[uniffi::export]
pub extern "C" fn ohc_init() -> *mut Handle {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    Box::into_raw(Box::new(Handle { tokio_rt }))
}

#[uniffi::export]
pub extern "C" fn ohc_free(handle: *mut Handle) {
    unsafe {
        drop(Box::from_raw(handle));
    };
}

#[derive(uniffi::Object)]
#[allow(unused)]
pub struct Handle {
    tokio_rt: tokio::runtime::Runtime,
}
