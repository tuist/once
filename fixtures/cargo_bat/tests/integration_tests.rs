use std::process::Command;

// Resolving `sh` by name needs a search path, which a test process only has
// when the runner gives it one.
#[test]
fn finds_a_system_program_on_the_search_path() {
    let output = Command::new("sh")
        .args(["-c", "printf ok"])
        .output()
        .expect("failed to run sh from the search path");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok");
}

#[test]
fn runs_the_package_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_bat"))
        .output()
        .expect("failed to run the bat binary");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3 bat");
}

// Cargo applies the `[env]` table from `.cargo/config.toml` to the processes
// it starts, and the compiler is one of them.
#[test]
fn sees_cargo_configuration_environment() {
    assert_eq!(env!("BAT_FIXTURE_ENV"), "from-cargo-config");
    assert_eq!(std::env::var("BAT_FIXTURE_ENV").as_deref(), Ok("from-cargo-config"));
}
