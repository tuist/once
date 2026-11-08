#shellcheck shell=bash

Describe 'Cargo build-script sibling source inputs'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  script_cache_is() {
    once --format json query evidence backend-0.1.0:build --limit 100 |
      jq -e --arg expected "$1" '
        map(select(any(.outputs | keys[]; endswith("/build-script.stdout")))) |
        max_by(.created_at_unix_ms).cache == $expected' >/dev/null
  }

  create_source_fixture() {
    mkdir -p "$WORKSPACE/src" "$SPEC_ROOT/packages/backend/src" "$SPEC_ROOT/packages/graphics/src"
    cat > "$WORKSPACE/Cargo.toml" <<TOML
[package]
name = "app"
version = "0.1.0"
edition = "2021"
[dependencies]
backend = { path = "$SPEC_ROOT/packages/backend" }
graphics = { path = "$SPEC_ROOT/packages/graphics" }
TOML
    cat > "$SPEC_ROOT/packages/backend/Cargo.toml" <<'TOML'
[package]
name = "backend"
version = "0.1.0"
edition = "2021"
TOML
    cat > "$SPEC_ROOT/packages/graphics/Cargo.toml" <<'TOML'
[package]
name = "graphics"
version = "0.2.2"
edition = "2021"
TOML
    printf 'pub fn unused() {}\n' > "$SPEC_ROOT/packages/graphics/src/lib.rs"
    printf 'one' > "$SPEC_ROOT/packages/graphics/src/scene.rs"
    printf 'own' > "$SPEC_ROOT/packages/backend/src/renderer.txt"
    printf 'shader' > "$SPEC_ROOT/packages/backend/src/shaders.metal"
    cat > "$SPEC_ROOT/packages/backend/build.rs" <<'RS'
use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let sibling = fs::read_to_string(root.join("../graphics/src/scene.rs")).unwrap();
    let own = fs::read_to_string(root.join("src/renderer.txt")).unwrap();
    let shader = fs::read_to_string(root.join("src/shaders.metal")).unwrap();
    println!("cargo:rustc-env=GENERATED={sibling}:{own}:{shader}");
}
RS
    printf 'pub fn value() -> &\x27static str { env!("GENERATED") }\n' > "$SPEC_ROOT/packages/backend/src/lib.rs"
    printf 'fn main() { println!("{}", backend::value()); }\n' > "$WORKSPACE/src/main.rs"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "cargo"
kind = "cargo_workspace"
srcs = ["Cargo.toml", "Cargo.lock"]
[target.attrs.build_script_sources."backend@0.1.0"]
"../graphics/src/scene.rs" = "graphics@0.2.2/src/scene.rs"
TOML
  }

  exercise_source_inputs() {
    kind="$1"
    create_source_fixture
    source_root="$SPEC_ROOT/packages"
    binary="$WORKSPACE/.once/out/cargo_app_bin_app/app"
    if [ "$kind" = cargo_dependencies ]; then
      mkdir -p "$WORKSPACE/vendor"
      cp -R "$SPEC_ROOT/packages/backend" "$WORKSPACE/vendor/backend-0.1.0"
      cp -R "$SPEC_ROOT/packages/graphics" "$WORKSPACE/vendor/graphics-0.2.2"
      source_root="$WORKSPACE/vendor"
      cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "deps"
kind = "cargo_dependencies"
srcs = ["Cargo.toml", "Cargo.lock"]
[target.attrs]
vendor_dir = "vendor"
[target.attrs.build_script_sources."backend@0.1.0"]
"../graphics/src/scene.rs" = "graphics@0.2.2/src/scene.rs"

[[target]]
name = "app"
kind = "rust_binary"
srcs = ["src/main.rs"]
deps = ["deps"]
[target.attrs]
cargo_package = "app"
TOML
      binary="$WORKSPACE/.once/out/app/app"
      scene="$source_root/graphics-0.2.2/src/scene.rs"
      renderer="$source_root/backend-0.1.0/src/renderer.txt"
    else
      scene="$source_root/graphics/src/scene.rs"
      renderer="$source_root/backend/src/renderer.txt"
    fi
    once build backend-0.1.0 --quiet >/dev/null || return
    script_cache_is miss || return
    once build --quiet >/dev/null || return
    "$binary" || return
    once build backend-0.1.0 --quiet >/dev/null || return
    script_cache_is hit || return
    once build --quiet >/dev/null || return
    "$binary" || return
    printf 'two' > "$scene"
    once build backend-0.1.0 --quiet >/dev/null || return
    script_cache_is miss || return
    once build --quiet >/dev/null || return
    "$binary" || return
    printf 'changed' > "$renderer"
    once build backend-0.1.0 --quiet >/dev/null || return
    script_cache_is miss || return
    once build --quiet >/dev/null || return
    "$binary" || return
    once build backend-0.1.0 --quiet >/dev/null || return
    script_cache_is hit || return
    once build --quiet >/dev/null || return
    "$binary" || return
    printf 'pub fn unrelated() {}\n' >> "$(dirname "$scene")/lib.rs"
    once build backend-0.1.0 --quiet >/dev/null || return
    script_cache_is hit || return
    if [ "$kind" = cargo_dependencies ]; then
      test ! -e "$WORKSPACE/vendor/graphics" || return
      test "$(cat "$SPEC_ROOT/packages/graphics/src/scene.rs")" = one || return
      test "$(cat "$SPEC_ROOT/packages/backend/src/renderer.txt")" = own || return
    fi
    test "$(cat "$SPEC_ROOT/packages/backend/src/shaders.metal")" = shader
  }

  exercise_removed_destination() {
    create_source_fixture
    once build backend-0.1.0 --quiet >/dev/null || return
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "cargo"
kind = "cargo_workspace"
srcs = ["Cargo.toml", "Cargo.lock"]
[target.attrs.build_script_sources."backend@0.1.0"]
"../graphics/src/other.rs" = "graphics@0.2.2/src/scene.rs"
TOML
    for state in warm cold; do
      if [ "$state" = cold ]; then
        rm -rf "$WORKSPACE/.once"
      fi
      if once build backend-0.1.0 --quiet > "$SPEC_ROOT/failure.log" 2>&1; then
        return 1
      fi
      renamed="$(find "$WORKSPACE/.once/out/backend-0.1.0/source-layout" -name other.rs)"
      test -f "$renamed" || return
      test ! -e "$(dirname "$renamed")/scene.rs" || return
      grep -q 'No such file or directory' "$SPEC_ROOT/failure.log" || return
    done
  }

  It 'does not reuse removed destinations in warm layouts or cache restores'
    When call exercise_removed_destination
    The status should be success
    The output should equal ''
  End

  It 'snapshots explicit sibling files and invalidates warm cargo_workspace builds'
    When call exercise_source_inputs cargo_workspace
    The status should be success
    The output should equal 'one:own:shader
one:own:shader
two:own:shader
two:changed:shader
two:changed:shader'
  End

  It 'maps versioned cargo_dependencies vendor directories without modifying them'
    When call exercise_source_inputs cargo_dependencies
    The status should be success
    The output should equal 'one:own:shader
one:own:shader
two:own:shader
two:changed:shader
two:changed:shader'
  End
End
