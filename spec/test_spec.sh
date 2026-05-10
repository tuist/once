#shellcheck shell=bash
# End-to-end specs for `fabrik test`.

Describe 'fabrik test'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  rust_test_workspace_toml() {
    mkdir -p "$WORKSPACE/crates/math/src" "$WORKSPACE/crates/math/tests"
    cat > "$WORKSPACE/crates/math/src/lib.rs" <<'EOF'
pub fn add(left: i32, right: i32) -> i32 {
    left + right
}
EOF
    cat > "$WORKSPACE/crates/math/tests/math.rs" <<'EOF'
extern crate math;

#[test]
fn adds_numbers() {
    assert_eq!(math::add(2, 3), 5);
}
EOF
    cat > "$WORKSPACE/crates/math/fabrik.toml" <<'EOF'
[[rust.library]]
name = "math"
srcs = ["src/lib.rs"]

[[rust.test]]
name = "math_test"
srcs = ["tests/math.rs"]
crate_root = "tests/math.rs"
deps = ["crates/math/math"]
EOF
  }

  It 'builds and runs a rust_test target declared in fabrik.toml'
    rust_test_workspace_toml
    When call fabrik test crates/math/math_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'test cache miss'
    The stderr should include 'build 2 nodes'
  End

  It 'reuses the cached test result on a second invocation'
    rust_test_workspace_toml
    fabrik test crates/math/math_test >/dev/null 2>&1
    When call fabrik test crates/math/math_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'test cache hit'
  End

  It 'passes arguments through to the Rust test harness'
    rust_test_workspace_toml
    When call fabrik test crates/math/math_test -- --list
    The status should be success
    The stdout should include 'adds_numbers: test'
    The stderr should include 'test cache miss'
  End

  It 'propagates failing test status'
    mkdir -p "$WORKSPACE/failing/tests"
    cat > "$WORKSPACE/failing/tests/failing.rs" <<'EOF'
#[test]
fn fails() {
    assert_eq!(1, 2);
}
EOF
    cat > "$WORKSPACE/failing/fabrik.toml" <<'EOF'
[[rust.test]]
name = "failing_test"
srcs = ["tests/failing.rs"]
crate_root = "tests/failing.rs"
EOF
    When call fabrik test failing/failing_test
    The status should not equal 0
    The stdout should include 'test result: FAILED'
    The stderr should include 'exit=101'
  End

  It 'runs a rust_test target declared in a nested package'
    mkdir -p "$WORKSPACE/pkg/tests"
    cat > "$WORKSPACE/pkg/tests/pkg.rs" <<'EOF'
#[test]
fn package_test_runs() {
    assert!(true);
}
EOF
    cat > "$WORKSPACE/pkg/fabrik.toml" <<'EOF'
[[rust.test]]
name = "pkg_test"
srcs = ["tests/pkg.rs"]
crate_root = "tests/pkg.rs"
EOF
    When call fabrik test pkg/pkg_test
    The status should be success
    The stdout should include 'package_test_runs'
    The stderr should include 'test cache miss'
  End

  It 'errors when the target is not a rust_test'
    mkdir -p "$WORKSPACE/lib/src"
    : > "$WORKSPACE/lib/src/lib.rs"
    cat > "$WORKSPACE/lib/fabrik.toml" <<'EOF'
[[rust.library]]
name = "lib"
srcs = ["src/lib.rs"]
EOF
    When call fabrik test lib/lib
    The status should not equal 0
    The stderr should include 'expected rust_test'
  End
End
