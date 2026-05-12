#shellcheck shell=bash
# End-to-end specs for `fabrik build` (granular per-crate pipeline).
#
# These specs verify the contract that distinguishes `fabrik build`
# from `fabrik run`: every crate is its own action, edits to one crate
# only invalidate that crate's cache slot (and reverse deps), and a
# cache hit restores the produced artifacts from the CAS even if they
# were wiped from the build output directory.

Describe 'fabrik build'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  granular_workspace() {
    mkdir -p "$WORKSPACE/crates/say/src" "$WORKSPACE/crates/hello/src"
    cat > "$WORKSPACE/crates/say/src/lib.rs" <<'EOF'
pub fn message() -> &'static str { "hello" }
EOF
    cat > "$WORKSPACE/crates/say/fabrik.toml" <<'EOF'
[[rust.library]]
name = "say"
srcs = ["src/lib.rs"]
EOF
    cat > "$WORKSPACE/crates/hello/src/main.rs" <<'EOF'
fn main() { println!("{}", say::message()); }
EOF
    cat > "$WORKSPACE/crates/hello/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "hello"
srcs = ["src/main.rs"]
deps = ["crates/say/say"]
EOF
  }

  build_script_workspace() {
    mkdir -p "$WORKSPACE/examples/rust/granular"
    cp -R \
      "$REPO_ROOT/examples/rust/granular/build-script-cfg" \
      "$WORKSPACE/examples/rust/granular/build-script-cfg"
  }

  It 'compiles a binary that depends on a library, one rustc per crate'
    granular_workspace
    When call fabrik build crates/hello/hello
    The status should be success
    The stderr should include 'rust_library'
    The stderr should include 'rust_binary'
    The stderr should include '2 nodes, 0 hit, 2 miss'
    The path "$WORKSPACE/.fabrik/out/crates/hello/hello" should be exist
  End

  It 'compiles a Rust binary declared in fabrik.toml'
    mkdir -p "$WORKSPACE/hello/src"
    cat > "$WORKSPACE/hello/src/main.rs" <<'EOF'
fn main() { println!("hello toml"); }
EOF
    cat > "$WORKSPACE/hello/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "hello"
srcs = ["src/main.rs"]
EOF
    When call fabrik build hello/hello
    The status should be success
    The stderr should include 'rust_binary'
    The path "$WORKSPACE/.fabrik/out/hello/hello" should be exist
  End

  It 'threads cargo build-script cfgs into dependent rustc actions'
    build_script_workspace
    fabrik build examples/rust/granular/build-script-cfg/app >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/examples/rust/granular/build-script-cfg/app"
    The status should be success
    The stdout should equal 'enabled'
  End

  It 'reuses the cache for a build-script-backed Rust graph'
    build_script_workspace
    fabrik build examples/rust/granular/build-script-cfg/app >/dev/null 2>&1
    When call fabrik build examples/rust/granular/build-script-cfg/app
    The status should be success
    The stderr should include '3 nodes, 3 hit, 0 miss'
  End

  It 'invalidates build-script dependents when a declared script input changes'
    build_script_workspace
    fabrik build examples/rust/granular/build-script-cfg/app >/dev/null 2>&1
    cat > "$WORKSPACE/examples/rust/granular/build-script-cfg/flag.txt" <<'EOF'
disabled
EOF
    When call fabrik build examples/rust/granular/build-script-cfg/app
    The status should be success
    The stderr should include '3 nodes, 0 hit, 3 miss'
  End

  It 'builds an iOS simulator app bundle declared in fabrik.toml'
    mkdir -p "$WORKSPACE/bin" "$WORKSPACE/App/Sources"
    cat > "$WORKSPACE/bin/xcrun" <<'EOF'
#!/bin/sh
if [ "$1" = "--sdk" ] && [ "$3" = "--show-sdk-path" ]; then
  echo "/tmp/fake-iphonesimulator.sdk"
  exit 0
fi
if [ "$1" = "--sdk" ] && [ "$3" = "swiftc" ]; then
  shift 3
  exec swiftc "$@"
fi
echo "unexpected xcrun invocation: $*" >&2
exit 2
EOF
    cat > "$WORKSPACE/bin/swiftc" <<'EOF'
#!/bin/sh
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="$1"
  fi
  shift
done
mkdir -p "$(dirname "$out")"
printf 'fake app binary' > "$out"
EOF
    cat > "$WORKSPACE/bin/codesign" <<'EOF'
#!/bin/sh
for last do :; done
app="$last"
mkdir -p "$app/_CodeSignature"
printf 'signed' > "$app/_CodeSignature/CodeResources"
EOF
    chmod +x "$WORKSPACE/bin/xcrun" "$WORKSPACE/bin/swiftc" "$WORKSPACE/bin/codesign"
    cat > "$WORKSPACE/App/Sources/App.swift" <<'EOF'
import SwiftUI

@main
struct DemoApp: App {
    var body: some Scene {
        WindowGroup { Text("Hello") }
    }
}
EOF
    cat > "$WORKSPACE/App/fabrik.toml" <<'EOF'
[[apple.simulator_app]]
name = "Demo"
platform = "ios"
bundle_id = "dev.fabrik.demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
EOF
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build App/Demo
    The status should be success
    The stderr should include 'apple_simulator_app'
    The path "$WORKSPACE/.fabrik/out/App/Demo.app" should be directory
    The path "$WORKSPACE/.fabrik/out/App/Demo.app/Info.plist" should be file
    The path "$WORKSPACE/.fabrik/out/App/Demo.app/Demo" should be file
  End

  It 'produces a runnable binary that links against the library'
    granular_workspace
    fabrik build crates/hello/hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/crates/hello/hello"
    The status should be success
    The stdout should equal 'hello'
  End

  It 'second invocation is a full cache hit on every node'
    granular_workspace
    fabrik build crates/hello/hello >/dev/null 2>&1
    When call fabrik build crates/hello/hello
    The status should be success
    The stderr should include '2 nodes, 2 hit, 0 miss'
  End

  It 'invalidates only the binary when its source changes'
    granular_workspace
    fabrik build crates/hello/hello >/dev/null 2>&1
    cat > "$WORKSPACE/crates/hello/src/main.rs" <<'EOF'
fn main() { println!("PREFIX: {}", say::message()); }
EOF
    When call fabrik build crates/hello/hello
    The status should be success
    # Library is reused; binary recompiles.
    The stderr should include '1 hit'
    The stderr should include '1 miss'
  End

  It 'invalidates the dependent binary when the library changes'
    granular_workspace
    fabrik build crates/hello/hello >/dev/null 2>&1
    cat > "$WORKSPACE/crates/say/src/lib.rs" <<'EOF'
pub fn message() -> &'static str { "edited" }
EOF
    When call fabrik build crates/hello/hello
    The status should be success
    The stderr should include '0 hit, 2 miss'
  End

  It 'restores cached outputs from CAS after the out dir is wiped'
    # The granular cache is content-addressed, so a wipe-and-rebuild
    # must not need to re-run rustc. This is the property that makes
    # remote execution possible: a fresh checkout can populate every
    # output from a shared CAS without any compile work.
    granular_workspace
    fabrik build crates/hello/hello >/dev/null 2>&1
    rm -rf "$WORKSPACE/.fabrik/out"
    When call fabrik build crates/hello/hello
    The status should be success
    The stderr should include '2 nodes, 2 hit, 0 miss'
    The path "$WORKSPACE/.fabrik/out/crates/hello/hello" should be exist
  End

  It 'restored binary still runs after a CAS-only rebuild'
    granular_workspace
    fabrik build crates/hello/hello >/dev/null 2>&1
    rm -rf "$WORKSPACE/.fabrik/out"
    fabrik build crates/hello/hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/crates/hello/hello"
    The status should be success
    The stdout should equal 'hello'
  End

  It 'errors when the target id does not match a target'
    granular_workspace
    When call fabrik build crates/hello/nope
    The status should not equal 0
    The stderr should include 'no target matches'
  End
End
