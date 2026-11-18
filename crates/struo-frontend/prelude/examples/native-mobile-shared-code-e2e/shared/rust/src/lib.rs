#[no_mangle]
pub extern "C" fn struo_shared_answer() -> i32 {
    42
}

#[no_mangle]
pub extern "system" fn Java_dev_struo_shared_MainActivity_rustAnswer(
    _environment: *mut core::ffi::c_void,
    _instance: *mut core::ffi::c_void,
) -> i32 {
    42
}
