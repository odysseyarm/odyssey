pub mod client;
pub mod ffi_common;
pub mod funny;

#[no_mangle]
pub extern "C" fn odyssey_hub_client_init() -> Box<Handle> {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    Box::new(Handle { tokio_rt })
}

#[no_mangle]
pub extern "C" fn odyssey_hub_client_free(handle: *mut Handle) {
    unsafe {
        drop(Box::from_raw(handle));
    };
}

#[allow(unused)]
pub struct Handle {
    tokio_rt: tokio::runtime::Runtime,
}
