// Only helpers, so this target compiles and reports no cases, the same way
// Cargo treats a `tests/` file without any `#[test]` function.
#[allow(dead_code)]
pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hyperfine")
}
