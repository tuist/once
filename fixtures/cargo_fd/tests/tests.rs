use std::process::Command;

#[test]
fn runs_the_renamed_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_fd"))
        .arg("needle")
        .output()
        .expect("failed to run the fd binary");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "found needle");
}
