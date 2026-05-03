#shellcheck shell=bash
# End-to-end specs for `fabrik build`.

Describe 'fabrik build'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  hello_workspace() {
    mkdir -p "$WORKSPACE/hello/src"
    cat > "$WORKSPACE/hello/src/main.rs" <<'EOF'
fn main() {
    println!("hello fabrik");
}
EOF
    cat > "$WORKSPACE/hello/fabrik.star" <<'EOF'
rust_binary(name = "hello", srcs = glob(["src/*.rs"]))
EOF
  }

  It 'compiles a rust_binary and writes the binary under .fabrik/out'
    hello_workspace
    When call fabrik build //hello:hello
    The status should be success
    The stderr should include 'cache miss'
    The stderr should include 'exit=0'
    The path "$WORKSPACE/.fabrik/out/hello/hello" should be exist
  End

  It 'produces a runnable binary'
    hello_workspace
    fabrik build //hello:hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/hello/hello"
    The status should be success
    The stdout should equal 'hello fabrik'
  End

  It 'is a cache hit on a second invocation'
    hello_workspace
    fabrik build //hello:hello >/dev/null 2>&1
    When call fabrik build //hello:hello
    The status should be success
    The stderr should include 'cache hit'
  End

  It 'errors when the label does not match any target'
    hello_workspace
    When call fabrik build //hello:nope
    The status should not equal 0
    The stderr should include 'no target matches'
  End

  It 'errors on an unsupported target kind'
    mkdir -p "$WORKSPACE/lib"
    : > "$WORKSPACE/lib/lib.rs"
    cat > "$WORKSPACE/lib/fabrik.star" <<'EOF'
rust_library(name = "lib", srcs = ["lib.rs"])
EOF
    When call fabrik build //lib:lib
    The status should not equal 0
    The stderr should include 'rust_library'
  End
End
