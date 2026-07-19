#[no_mangle]
pub extern "C" fn once_shared_greeting_length() -> i32 {
    shared_core::greeting_length()
}
