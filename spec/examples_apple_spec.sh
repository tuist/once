#shellcheck shell=bash
# End-to-end specs for checked-in Apple examples.
#
# These specs require a real macOS toolchain (xcrun, swiftc, codesign).
# Each case starts with `Skip if "..." apple_toolchain_unavailable` so
# the suite stays green on non-Darwin runners without faking Xcode; the
# simulator-launch case additionally requires a booted simulator.

Describe 'Apple examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'builds the checked-in macOS Swift example with granular Apple actions'
    Skip if "no apple toolchain" apple_toolchain_unavailable
    copy_examples
    When call fabrik build examples/apple/macos/cli/hello
    The status should be success
    The stderr should include 'swift_compile'
    The stderr should include 'swift_archive'
    The stderr should include 'macos_command_line_application'
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/cli/Greeter/libGreeter.a" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/cli/Greeter/Greeter.swiftmodule" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/cli/hello" should be file
  End

  It 'reuses the cache for the checked-in macOS Swift example build'
    Skip if "no apple toolchain" apple_toolchain_unavailable
    copy_examples
    fabrik build examples/apple/macos/cli/hello >/dev/null 2>&1
    When call fabrik build examples/apple/macos/cli/hello
    The status should be success
    The stderr should include '3 nodes, 3 hit, 0 miss'
  End

  It 'builds a Tuist-shaped Swift module graph with cacheable internal actions'
    Skip if "no apple toolchain" apple_toolchain_unavailable
    copy_examples
    When call fabrik build examples/apple/macos/tuist-shaped-swift/tuist
    The status should be success
    The stderr should include 'swift_compile'
    The stderr should include 'swift_archive'
    The stderr should include 'macos_command_line_application'
    The stderr should include '13 nodes'
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/tuist-shaped-swift/TuistKit/libTuistKit.a" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/macos/tuist-shaped-swift/tuist" should be file
  End

  It 'reuses the cache for the Tuist-shaped Swift module graph'
    Skip if "no apple toolchain" apple_toolchain_unavailable
    copy_examples
    fabrik build examples/apple/macos/tuist-shaped-swift/tuist >/dev/null 2>&1
    When call fabrik build examples/apple/macos/tuist-shaped-swift/tuist
    The status should be success
    The stderr should include '13 nodes, 13 hit, 0 miss'
  End

  It 'builds the checked-in iOS example with the Apple plugin'
    Skip if "no apple toolchain" apple_toolchain_unavailable
    copy_examples
    When call fabrik build examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include 'apple_simulator_app'
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app" should be directory
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app/Info.plist" should be file
    The path "$WORKSPACE/.fabrik/out/examples/apple/ios/simulator-app/Demo.app/Demo" should be file
  End

  It 'reuses the cache for the checked-in iOS example build'
    Skip if "no apple toolchain" apple_toolchain_unavailable
    copy_examples
    fabrik build examples/apple/ios/simulator-app/Demo >/dev/null 2>&1
    When call fabrik build examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include '1 nodes, 1 hit, 0 miss'
  End

  It 'launches the checked-in iOS example through simctl'
    # Real simctl install/launch needs a booted simulator. CI macos
    # runners don't have one up by default; this case is opt-in for
    # local dev where a simulator is already running.
    Skip if "no booted iOS simulator" no_booted_ios_simulator
    copy_examples
    When call fabrik run examples/apple/ios/simulator-app/Demo
    The status should be success
    The stderr should include 'cache miss'
  End
End
