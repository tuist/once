#shellcheck shell=bash
# End-to-end specs for Rust graph targets.

Describe 'rust graph'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_failing_build_script_fixture() {
    mkdir -p "$WORKSPACE/crates/failing/src"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[workspace]
include = ["crates/*/once.toml"]
TOML
    cat > "$WORKSPACE/crates/failing/once.toml" <<'TOML'
[[target]]
name = "failing"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "failing"
crate_root = "src/lib.rs"
build_script = "build.rs"
edition = "2021"
TOML
    printf 'pub fn value() -> i32 { 1 }\n' > "$WORKSPACE/crates/failing/src/lib.rs"
    cat > "$WORKSPACE/crates/failing/build.rs" <<'RS'
fn main() {
    println!("build script stdout is visible");
    eprintln!("build script stderr is visible");
    std::process::exit(7);
}
RS
  }

  It 'surfaces captured build script output when a declared action fails'
    create_failing_build_script_fixture

    When call once build crates/failing/failing
    The status should be failure
    The stderr should include 'crates/failing/failing:build-script'
    The stderr should include 'stdout:'
    The stderr should include 'build script stdout is visible'
    The stderr should include 'build script stderr is visible'
    The stderr should not include 'last 16384 bytes of stderr'
  End

  It 'records evidence when a declared build action fails'
    create_failing_build_script_fixture
    once build crates/failing/failing >/dev/null 2>&1 || true

    When call once --format json query evidence crates/failing/failing:build
    The status should be success
    The stdout should include '"subject":{"kind":"target","id":"crates/failing/failing","capability":"build"}'
    The stdout should include '"status":"failed"'
    The stdout should include '"cache":"miss"'
    The stdout should include '"exit_code":7'
    The stdout should include '"stdout"'
    The stdout should include '"stderr"'
  End
End
