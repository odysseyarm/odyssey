pub mod client;
pub mod ffi_common;
pub mod funny;

#[no_mangle]
pub extern "C" fn init() -> Box<Handle> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let tokio_handle = rt.handle().clone();
    Box::new(Handle { tokio_handle })
}

#[no_mangle]
pub extern "C" fn free(handle: *mut Handle) {
    unsafe { drop(Box::from_raw(handle)); };
}

#[allow(unused)]
#[derive(Clone)]
pub struct Handle {
    tokio_handle: tokio::runtime::Handle,
}
