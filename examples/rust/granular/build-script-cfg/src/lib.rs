#[cfg(build_script_enabled)]
pub fn message() -> &'static str {
    "enabled"
}

#[cfg(not(build_script_enabled))]
pub fn message() -> &'static str {
    "disabled"
}
