pub mod client;
pub mod funny;
pub mod tracking_history;

#[allow(unused)]
pub struct Handle {
    tokio_rt: tokio::runtime::Runtime,
}

#[no_mangle]
pub extern "C" fn rt_init() -> *mut Handle {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    Box::into_raw(Box::new(Handle { tokio_rt }))
}

#[no_mangle]
pub extern "C" fn rt_free(handle: *mut Handle) {
    unsafe {
        drop(Box::from_raw(handle));
    };
}
