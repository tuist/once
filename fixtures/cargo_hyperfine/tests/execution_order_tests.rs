#[test]
fn keeps_a_second_test_file_separate() {
    assert!(!env!("CARGO_BIN_EXE_hyperfine").is_empty());
}
