use interoptopus::{ffi_function, ffi_type, function, inventory::{Inventory, InventoryBuilder}};

pub mod client;
pub mod ffi_common;
pub mod funny;

#[ffi_function]
pub extern "C" fn ohc_init() -> *mut Handle {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    Box::into_raw(Box::new(Handle { tokio_rt }))
}

#[ffi_function]
pub extern "C" fn ohc_free(handle: *mut Handle) {
    unsafe {
        drop(Box::from_raw(handle));
    };
}

#[ffi_type(skip(tokio_rt))]
#[allow(unused)]
pub struct Handle {
    tokio_rt: tokio::runtime::Runtime,
}

pub fn ffi_inventory() -> Inventory {
    InventoryBuilder::new()
        .register(function!(ohc_init))
        .register(function!(ohc_free))
        .register(function!(client::ohc_c_new))
        .register(function!(client::ohc_c_free))
        .register(function!(client::ohc_c_connect))
        .register(function!(client::ohc_c_get_device_list))
        .register(function!(client::ohc_c_start_stream))
        .register(function!(client::ohc_c_stop_stream))
        .register(function!(client::ohc_c_write_vendor))
        .register(function!(client::ohc_c_reset_zero))
        .register(function!(client::ohc_c_zero))
        .register(function!(client::ohc_c_get_screen_info_by_id))
        .validate()
        .validate()
        .build()
}
