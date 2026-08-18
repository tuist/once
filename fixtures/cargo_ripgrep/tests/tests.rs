use std::process::Command;

#[test]
fn runs_the_package_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_rg"))
        .arg("needle")
        .output()
        .expect("failed to run the rg binary");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "searching for needle");
}
