#shellcheck shell=bash
# End-to-end specs for checked-in Apple examples.

Describe 'Apple examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'builds the checked-in macOS Swift example with granular Apple actions'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/macos/cli/hello
    The status should be success
    The stderr should include 'swift_compile'
    The stderr should include 'swift_archive'
    The stderr should include 'macos_command_line_application'
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/cli/Greeter/libGreeter.a" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/cli/Greeter/Greeter.swiftmodule" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/cli/hello" should be file
  End

  It 'reuses the cache for the checked-in macOS Swift example build'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/macos/cli/hello >/dev/null 2>&1
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/macos/cli/hello
    The status should be success
    The stderr should include '3 nodes, 3 hit, 0 miss'
  End

  It 'builds a Tuist-shaped Swift module graph with cacheable internal actions'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/macos/tuist-shaped-swift/tuist
    The status should be success
    The stderr should include 'swift_compile'
    The stderr should include 'swift_archive'
    The stderr should include 'macos_command_line_application'
    The stderr should include '13 nodes'
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/tuist-shaped-swift/TuistKit/libTuistKit.a" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/tuist-shaped-swift/tuist" should be file
  End

  It 'reuses the cache for the Tuist-shaped Swift module graph'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/macos/tuist-shaped-swift/tuist >/dev/null 2>&1
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/macos/tuist-shaped-swift/tuist
    The status should be success
    The stderr should include '13 nodes, 13 hit, 0 miss'
  End

  It 'builds the checked-in iOS example with the Apple plugin'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include 'apple_simulator_app'
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app" should be directory
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app/Info.plist" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app/Demo" should be file
  End

  It 'reuses the cache for the checked-in iOS example build'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/ios/simulator-app/Demo >/dev/null 2>&1
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include '1 nodes, 1 hit, 0 miss'
  End

  It 'launches the checked-in iOS example through simctl'
    copy_examples
    fake_apple_tools
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" run examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl install booted'
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl launch booted dev.fabrik.ios-demo'
  End

  It 'keeps iOS launch uncached while reusing the cached app build'
    copy_examples
    fake_apple_tools
    env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" build examples/apple/ios/simulator-app/Demo >/dev/null 2>&1
    rm -f "$WORKSPACE/swiftc.log"
    When call env PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" run examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include 'cache miss'
    The path "$WORKSPACE/swiftc.log" should not be exist
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl install booted'
    The contents of file "$WORKSPACE/simctl.log" should include 'simctl launch booted dev.fabrik.ios-demo'
  End
End
