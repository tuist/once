#shellcheck shell=bash
# End-to-end specs for checked-in example projects.

Describe 'examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  copy_examples() {
    mkdir -p "$WORKSPACE/examples"
    cp -R "$REPO_ROOT/examples/." "$WORKSPACE/examples/"
  }

  fake_apple_tools() {
    mkdir -p "$WORKSPACE/bin"
    cat > "$WORKSPACE/bin/xcrun" <<'EOF'
#!/bin/sh
if [ "$1" = "--sdk" ] && [ "$3" = "--show-sdk-path" ]; then
  echo "/tmp/fake-$2.sdk"
  exit 0
fi
if [ "$1" = "--sdk" ] && [ "$3" = "swiftc" ]; then
  shift 3
  exec swiftc "$@"
fi
if [ "$1" = "ar" ] && [ "$2" = "crs" ]; then
  out="$3"
  mkdir -p "$(dirname "$out")"
  printf 'fake swift archive' > "$out"
  exit 0
fi
if [ "$1" = "simctl" ]; then
  echo "$*" >> simctl.log
  exit 0
fi
echo "unexpected xcrun invocation: $*" >&2
exit 2
EOF
    cat > "$WORKSPACE/bin/swiftc" <<'EOF'
#!/bin/sh
echo "swiftc $*" >> swiftc.log
out=""
module=""
emit_object=0
srcs=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="$1"
  elif [ "$1" = "-emit-module-path" ]; then
    shift
    module="$1"
  elif [ "$1" = "-emit-object" ]; then
    emit_object=1
  fi
  case "$1" in
    *.swift) srcs="$srcs $1" ;;
  esac
  shift
done
if [ -n "$out" ]; then
  mkdir -p "$(dirname "$out")"
  printf 'fake swift binary' > "$out"
fi
if [ "$emit_object" = 1 ]; then
  for src in $srcs; do
    object="$(basename "$src" .swift).o"
    printf 'fake swift object' > "$object"
  done
fi
if [ -n "$module" ]; then
  mkdir -p "$(dirname "$module")"
  printf 'fake swift module' > "$module"
fi
EOF
    cat > "$WORKSPACE/bin/codesign" <<'EOF'
#!/bin/sh
for last do :; done
app="$last"
mkdir -p "$app/_CodeSignature"
printf 'signed' > "$app/_CodeSignature/CodeResources"
EOF
    chmod +x "$WORKSPACE/bin/xcrun" "$WORKSPACE/bin/swiftc" "$WORKSPACE/bin/codesign"
  }

  It 'lists checked-in Rust and iOS examples'
    copy_examples
    When call fabrik targets
    The status should be success
    The stdout should include '//examples/rust/granular/basic-app:greeting'
    The stdout should include '//examples/rust/granular/basic-app:hello'
    The stdout should include '//examples/rust/granular/basic-app:greeting_test'
    The stdout should include '//examples/macos-cli:Greeter'
    The stdout should include '//examples/macos-cli:hello'
    The stdout should include '//examples/tuist-shaped-swift:tuist'
    The stdout should include '//examples/apple/ios/simulator-app:Demo'
  End

  It 'builds and runs the checked-in Rust example'
    copy_examples
    fabrik build //examples/rust/granular/basic-app:hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/examples/rust/granular/basic-app/hello"
    The status should be success
    The stdout should equal 'Hello, Rust, from Fabrik'
  End

  It 'reuses the cache for the checked-in Rust example build'
    copy_examples
    fabrik build //examples/rust/granular/basic-app:hello >/dev/null 2>&1
    When call fabrik build //examples/rust/granular/basic-app:hello
    The status should be success
    The stderr should include '2 nodes, 2 hit, 0 miss'
  End

  It 'builds and runs tests from the checked-in Rust example'
    copy_examples
    When call fabrik test //examples/rust/granular/basic-app:greeting_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'build 2 nodes'
  End

  It 'reuses the cache for the checked-in Rust example tests'
    copy_examples
    fabrik test //examples/rust/granular/basic-app:greeting_test >/dev/null 2>&1
    When call fabrik test //examples/rust/granular/basic-app:greeting_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'test cache hit'
  End

  It 'builds the checked-in macOS Swift example with granular Apple actions'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/macos-cli:hello
    The status should be success
    The stderr should include 'swift_compile'
    The stderr should include 'swift_archive'
    The stderr should include 'macos_command_line_application'
    The path "$WORKSPACE/.fabrik/out/examples/macos-cli/Greeter/libGreeter.a" should be file
    The path "$WORKSPACE/.fabrik/out/examples/macos-cli/Greeter/Greeter.swiftmodule" should be file
    The path "$WORKSPACE/.fabrik/out/examples/macos-cli/hello" should be file
  End

  It 'reuses the cache for the checked-in macOS Swift example build'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/macos-cli:hello >/dev/null 2>&1
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/macos-cli:hello
    The status should be success
    The stderr should include '3 nodes, 3 hit, 0 miss'
  End

  It 'builds a Tuist-shaped Swift module graph with cacheable internal actions'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/tuist-shaped-swift:tuist
    The status should be success
    The stderr should include 'swift_compile'
    The stderr should include 'swift_archive'
    The stderr should include 'macos_command_line_application'
    The stderr should include '13 nodes'
    The path "$WORKSPACE/.fabrik/out/examples/tuist-shaped-swift/TuistKit/libTuistKit.a" should be file
    The path "$WORKSPACE/.fabrik/out/examples/tuist-shaped-swift/tuist" should be file
  End

  It 'reuses the cache for the Tuist-shaped Swift module graph'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/tuist-shaped-swift:tuist >/dev/null 2>&1
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/tuist-shaped-swift:tuist
    The status should be success
    The stderr should include '13 nodes, 13 hit, 0 miss'
  End

  It 'builds the checked-in iOS example with the Apple plugin'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/apple/ios/simulator-app:Demo
    The status should be success
    The stderr should include 'apple_ios_app'
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app" should be directory
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app/Info.plist" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app/Demo" should be file
  End

  It 'reuses the cache for the checked-in iOS example build'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/apple/ios/simulator-app:Demo >/dev/null 2>&1
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/apple/ios/simulator-app:Demo
    The status should be success
    The stderr should include '1 nodes, 1 hit, 0 miss'
  End

  It 'launches the checked-in iOS example through simctl'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" run //examples/apple/ios/simulator-app:Demo
    The status should be success
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl install booted'
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl launch booted dev.fabrik.ios-demo'
  End

  It 'keeps iOS launch uncached while reusing the cached app build'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build //examples/apple/ios/simulator-app:Demo >/dev/null 2>&1
    rm -f "$WORKSPACE/swiftc.log"
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" run //examples/apple/ios/simulator-app:Demo
    The status should be success
    The stderr should include 'cache miss'
    The path "$WORKSPACE/swiftc.log" should not be exist
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl install booted'
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl launch booted dev.fabrik.ios-demo'
  End
End
