use std::process::Command;

#[test]
fn prints_a_summary() {
    let output = Command::new(env!("CARGO_BIN_EXE_hyperfine"))
        .output()
        .expect("failed to run the hyperfine binary");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3 runs");
}
