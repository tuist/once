#shellcheck shell=bash
# End-to-end specs for Rust graph targets.

Describe 'rust graph'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  rust_toolchain_unavailable() {
    command -v cargo >/dev/null 2>&1 || return 0
    command -v rustc >/dev/null 2>&1 || return 0
    return 1
  }

  create_generated_native_fixture() {
    mkdir -p "$WORKSPACE/crates/helper/src" "$WORKSPACE/crates/native/src" "$WORKSPACE/crates/app/src"
    cat > "$WORKSPACE/struo.toml" <<'TOML'
[workspace]
include = ["crates/*/struo.toml"]
TOML
    cat > "$WORKSPACE/crates/helper/struo.toml" <<'TOML'
[[target]]
name = "helper"
kind = "rust_library"
srcs = ["src/**/*.rs"]
TOML
    printf 'pub fn value() -> u32 { 7 }\npub fn offset() -> u32 { 1 }\n' > "$WORKSPACE/crates/helper/src/lib.rs"
    cat > "$WORKSPACE/crates/native/struo.toml" <<'TOML'
[[target]]
name = "native"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.dependencies]
build_deps = ["../helper/helper"]

[target.attrs]
build_script = "build.rs"
build_script_tools = ["cc", "ar"]
rustc_env = { CARGO_MANIFEST_LINKS = "generated" }
TOML
    cat > "$WORKSPACE/crates/native/build.rs" <<'RS'
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("generated.rs"), format!("pub const OFFSET: u32 = {};\n", helper::offset())).unwrap();
    fs::write(out.join("native.c"), format!("unsigned generated_value(void) {{ return {}; }}\n", helper::value())).unwrap();
    assert!(Command::new("cc").arg("-c").arg(out.join("native.c")).arg("-o").arg(out.join("native.o")).status().unwrap().success());
    assert!(Command::new("ar").env("ZERO_AR_DATE", "1").arg("crs").arg(out.join("libgenerated.a")).arg(out.join("native.o")).status().unwrap().success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static:-bundle=generated");
    println!("cargo:include={}", out.display());
}
RS
    cat > "$WORKSPACE/crates/native/src/lib.rs" <<'RS'
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
extern "C" { fn generated_value() -> u32; }
pub fn value() -> u32 { (unsafe { generated_value() }) + OFFSET }
RS
    cat > "$WORKSPACE/crates/app/struo.toml" <<'TOML'
[[target]]
name = "app"
kind = "rust_binary"
srcs = ["src/**/*.rs"]
deps = ["../native/native"]

[target.attrs]
build_script = "build.rs"
TOML
    cat > "$WORKSPACE/crates/app/build.rs" <<'RS'
use std::{env, fs, path::PathBuf};

fn main() {
    let include = PathBuf::from(env::var_os("DEP_GENERATED_INCLUDE").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy(include.join("generated.rs"), out.join("generated.rs")).unwrap();
}
RS
    cat > "$WORKSPACE/crates/app/src/main.rs" <<'RS'
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
fn main() { println!("{}", native::value() + OFFSET); }
RS
  }

  rebuild_with_unchanged_generated_files() {
    struo build crates/native/native --quiet >/dev/null || return
    printf 'pub fn value() -> u32 { 7 }\npub fn offset() -> u32 { 1 }\npub fn unrelated() -> u32 { 99 }\n' > "$WORKSPACE/crates/helper/src/lib.rs"
    struo build crates/native/native --quiet >/dev/null || return
    struo --format json query evidence crates/native/native:build --limit 100 |
      jq -r '
        def compilation: any(.outputs | keys[]; endswith(".rlib"));
        def generation: any(.outputs | keys[]; endswith("/build-script.stdout"));
        (map(select(generation and (compilation | not))) | max_by(.created_at_unix_ms).cache),
        (map(select(compilation and (generation | not))) | max_by(.created_at_unix_ms).cache)'
  }

  rebuild_with_changed_native_library() {
    struo build crates/app/app --quiet >/dev/null || return
    "$WORKSPACE/.struo/out/crates/app/app/app" || return
    printf 'pub fn value() -> u32 { 23 }\npub fn offset() -> u32 { 1 }\n' > "$WORKSPACE/crates/helper/src/lib.rs"
    struo build crates/app/app --quiet >/dev/null || return
    "$WORKSPACE/.struo/out/crates/app/app/app" || return
    printf 'pub fn value() -> u32 { 23 }\npub fn offset() -> u32 { 100 }\n' > "$WORKSPACE/crates/helper/src/lib.rs"
    struo build crates/app/app --quiet >/dev/null || return
    "$WORKSPACE/.struo/out/crates/app/app/app" || return
    rm -rf "$WORKSPACE/.struo/out"
    struo --format json build crates/app/app > "$SPEC_ROOT/restored.json" || return
    jq -r '.cache' "$SPEC_ROOT/restored.json" || return
    "$WORKSPACE/.struo/out/crates/app/app/app"
  }

  It 'reuses Rust compilation when a rebuilt generator produces identical files'
    rust_toolchain_unavailable && Skip 'cargo and rustc are required'
    create_generated_native_fixture

    When call rebuild_with_unchanged_generated_files
    The status should be success
    The stdout should equal "$(printf 'miss\nhit')"
  End

  It 'rebuilds changed generated Rust and native libraries and restores them from cache'
    rust_toolchain_unavailable && Skip 'cargo and rustc are required'
    create_generated_native_fixture

    When call rebuild_with_changed_native_library
    The status should be success
    The stdout should equal "$(printf '9\n25\n223\nhit\n223')"
  End

  create_failing_build_script_fixture() {
    mkdir -p "$WORKSPACE/crates/failing/src"
    cat > "$WORKSPACE/struo.toml" <<'TOML'
[workspace]
include = ["crates/*/struo.toml"]
TOML
    cat > "$WORKSPACE/crates/failing/struo.toml" <<'TOML'
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

    When call struo build crates/failing/failing
    The status should be failure
    The stderr should include 'crates/failing/failing:build-script'
    The stderr should include 'stdout:'
    The stderr should include 'build script stdout is visible'
    The stderr should include 'build script stderr is visible'
    The stderr should not include 'last 16384 bytes of stderr'
  End

  It 'records evidence when a declared build action fails'
    create_failing_build_script_fixture
    struo build crates/failing/failing >/dev/null 2>&1 || true

    When call struo --format json query evidence crates/failing/failing:build
    The status should be success
    The stdout should include '"subject":{"kind":"target","id":"crates/failing/failing","capability":"build"}'
    The stdout should include '"status":"failed"'
    The stdout should include '"cache":"miss"'
    The stdout should include '"exit_code":7'
    The stdout should include '"stdout"'
    The stdout should include '"stderr"'
  End

  rebuild_embedded_source() {
    mkdir -p "$WORKSPACE/src"
    cat > "$WORKSPACE/struo.toml" <<'TOML'
[[target]]
name = "embedded"
kind = "rust_library"
srcs = ["src/**"]

[target.attrs]
crate_name = "embedded"

[[target]]
name = "app"
kind = "rust_binary"
srcs = ["main.rs"]
deps = ["./embedded"]

[target.attrs]
crate_root = "main.rs"
TOML
    cat > "$WORKSPACE/src/lib.rs" <<'RS'
pub fn message() -> &'static str { include_str!("message.txt") }
RS
    printf 'fn main() { print!("{}", embedded::message()); }\n' > "$WORKSPACE/main.rs"
    printf 'original payload\n' > "$WORKSPACE/src/message.txt"
    struo build app > "$WORKSPACE/first-build.log" 2>&1 || { cat "$WORKSPACE/first-build.log" >&2; return 1; }
    "$WORKSPACE/.struo/out/app/app" || return 1
    printf 'updated payload\n' > "$WORKSPACE/src/message.txt"
    struo build app > "$WORKSPACE/second-build.log" 2>&1 || { cat "$WORKSPACE/second-build.log" >&2; return 1; }
    "$WORKSPACE/.struo/out/app/app"
  }

  It 'rebuilds consumers when a non-Rust source embedded by a library changes'
    Skip if 'Rust toolchain is unavailable' rust_toolchain_unavailable

    When call rebuild_embedded_source
    The status should be success
    The line 1 of stdout should equal 'original payload'
    The line 2 of stdout should equal 'updated payload'
  End
End
