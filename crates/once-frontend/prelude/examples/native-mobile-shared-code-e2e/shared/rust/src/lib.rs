#[no_mangle]
pub extern "C" fn once_shared_answer() -> i32 {
    42
}

#[no_mangle]
pub extern "system" fn Java_dev_once_shared_MainActivity_rustAnswer(
    _environment: *mut core::ffi::c_void,
    _instance: *mut core::ffi::c_void,
) -> i32 {
    42
}
