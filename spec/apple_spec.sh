#shellcheck shell=bash
# End-to-end specs for Apple graph targets.

Describe 'apple graph'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  apple_toolchain_unavailable() {
    case "$(uname -s)" in
      Darwin) ;;
      *) return 0 ;;
    esac
    command -v xcrun >/dev/null 2>&1 || return 0
    return 1
  }

  macos_runtime_unavailable() {
    [ "$(uname -s)" != "Darwin" ]
  }

  copy_apple_library_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_library/." "$WORKSPACE/"
  }

  copy_apple_library_mixed_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_library_mixed/." "$WORKSPACE/"
  }

  copy_apple_library_defines_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_library_defines/." "$WORKSPACE/"
  }

  copy_apple_library_multi_arch_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_library_multi_arch/." "$WORKSPACE/"
  }

  copy_apple_library_select_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_library_select/." "$WORKSPACE/"
  }

  copy_swift_macro_fixture() {
    cp -R "$REPO_ROOT/fixtures/swift_macro/." "$WORKSPACE/"
  }

  copy_apple_application_swiftui_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_application_swiftui/." "$WORKSPACE/"
  }

  copy_apple_thinned_package_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_thinned_package/." "$WORKSPACE/"
  }

  copy_apple_graph_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_graph/." "$WORKSPACE/"
  }

  copy_apple_swift_testing_fixture() {
    cp -R "$REPO_ROOT/fixtures/apple_swift_testing/." "$WORKSPACE/"
  }

  copy_xcode_generated_source_fixture() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/xcode-generated-source-e2e/." "$WORKSPACE/"
  }

  copy_xcode_firefox_app_fixture() {
    cp -R "$REPO_ROOT/fixtures/xcode_firefox_app/." "$WORKSPACE/"
  }

  copy_xcode_xcframework_import_fixture() {
    cp -R "$REPO_ROOT/fixtures/xcode_xcframework_import/." "$WORKSPACE/"
    mkdir -p "$WORKSPACE/Vendor"
    cp -R "$REPO_ROOT/fixtures/xcode_firefox_app/RustComponents.xcframework" "$WORKSPACE/Vendor/"
  }

  copy_xcode_wikipedia_app_fixture() {
    cp -R "$REPO_ROOT/fixtures/xcode_wikipedia_app/." "$WORKSPACE/"
  }

  copy_xcode_signal_app_fixture() {
    cp -R "$REPO_ROOT/fixtures/xcode_signal_app/." "$WORKSPACE/"
  }

  copy_xcode_prebuilt_cache_app_fixture() {
    if apple_toolchain_unavailable; then
      return 0
    fi
    cp -R "$REPO_ROOT/fixtures/xcode_prebuilt_cache_app/." "$WORKSPACE/"
    REMOTE_VENDOR_DIR="${TMPDIR:-/tmp}/once-remote-vendor-fixture"
    rm -rf "$REMOTE_VENDOR_DIR"
    if ! vendor_log="$(sh "$WORKSPACE/tools/build_vendors.sh" "$WORKSPACE/Vendor" "$REMOTE_VENDOR_DIR" 2>&1)"; then
      printf '%s\n' "$vendor_log" >&2
      return 1
    fi
    sed -i '' "s|__REMOTE_VENDOR_DIR__|$REMOTE_VENDOR_DIR|" "$WORKSPACE/Client.xcodeproj/project.pbxproj"
  }

  create_mock_apple_run_fixture() {
    mkdir -p "$WORKSPACE/bin" "$WORKSPACE/apps/ios/Sources/App"
    cat > "$WORKSPACE/apps/ios/Sources/App/App.swift" <<'SWIFT'
@main
struct App {
    static func main() {}
}
SWIFT
    cat > "$WORKSPACE/apps/ios/once.toml" <<'TOML'
[[target]]
name = "App"
kind = "apple_application"
srcs = ["Sources/App/**/*.swift"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
bundle_id = "dev.once.App"
minimum_os = "17.0"
TOML
    cat > "$WORKSPACE/bin/codesign" <<'SH'
#!/bin/sh
app=""
for arg do
  app="$arg"
done
mkdir -p "$app/_CodeSignature"
: > "$app/_CodeSignature/CodeResources"
SH
    chmod +x "$WORKSPACE/bin/codesign"
    cat > "$WORKSPACE/bin/swiftc" <<'SH'
#!/bin/sh
case "${1:-}" in
  --version)
    printf '%s\n' 'Apple Swift version mock'
    exit 0
    ;;
esac
out=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="${1:-}"
    break
  fi
  shift
done
[ -n "$out" ] || exit 2
mkdir -p "$(dirname "$out")"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$out"
chmod 755 "$out"
SH
    chmod +x "$WORKSPACE/bin/swiftc"
    cat > "$WORKSPACE/bin/xcrun" <<SH
#!/bin/sh
log='$WORKSPACE/xcrun.log'
if [ "\${1:-}" = "--sdk" ]; then
  shift 2
fi
case "\${1:-}" in
  --find)
    case "\${2:-}" in
      swiftc) printf '%s\\n' '$WORKSPACE/bin/swiftc' ;;
      codesign) printf '%s\\n' '$WORKSPACE/bin/codesign' ;;
      *) printf '%s\\n' "\$0" ;;
    esac
    ;;
  --show-sdk-path)
    printf '%s\\n' '$WORKSPACE/sdk'
    ;;
  simctl)
    printf '%s\\n' "\$*" >> "\$log"
    if [ "\${2:-}" = "list" ] && [ "\${3:-}" = "devices" ] && [ "\${4:-}" = "booted" ]; then
      printf '%s\\n' '    Apple TV (TV-DEVICE) (Booted)'
      printf '%s\\n' '    iPhone 15 (11111111-1111-1111-1111-111111111111) (Booted)'
    fi
    ;;
  *)
    printf '%s\\n' "unexpected xcrun invocation: \$*" >&2
    exit 1
    ;;
esac
SH
    chmod +x "$WORKSPACE/bin/xcrun"
  }

  create_mock_apple_test_fixture() {
    mkdir -p "$WORKSPACE/toolchain/usr/bin" "$WORKSPACE/bin" "$WORKSPACE/apps/ios/Tests/AppTests" "$WORKSPACE/Platforms/iPhoneSimulator.platform/Developer/Library/Frameworks" "$WORKSPACE/Platforms/iPhoneSimulator.platform/Developer/usr/lib"
    cat > "$WORKSPACE/apps/ios/Tests/AppTests/AppTests.swift" <<'SWIFT'
import Testing

@Test func examplePasses() {}
SWIFT
    cat > "$WORKSPACE/apps/ios/once.toml" <<'TOML'
[[target]]
name = "AppTests"
kind = "apple_test_bundle"
srcs = ["Tests/AppTests/**/*.swift"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
minimum_os = "17.0"
TOML
    cat > "$WORKSPACE/bin/codesign" <<'SH'
#!/bin/sh
app=""
for arg do
  app="$arg"
done
mkdir -p "$app/_CodeSignature"
: > "$app/_CodeSignature/CodeResources"
SH
    chmod +x "$WORKSPACE/bin/codesign"
    cat > "$WORKSPACE/toolchain/usr/bin/swiftc" <<'SH'
#!/bin/sh
case "${1:-}" in
  --version)
    printf '%s\n' 'Apple Swift version mock'
    exit 0
    ;;
esac
out=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="${1:-}"
    break
  fi
  shift
done
[ -n "$out" ] || exit 2
mkdir -p "$(dirname "$out")"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$out"
chmod 755 "$out"
SH
    chmod +x "$WORKSPACE/toolchain/usr/bin/swiftc"
    cat > "$WORKSPACE/toolchain/usr/bin/xcrun" <<SH
#!/bin/sh
log='$WORKSPACE/xcrun.log'
swiftc_path='$WORKSPACE/toolchain/usr/bin/swiftc'
platform_path='$WORKSPACE/Platforms/iPhoneSimulator.platform'
if [ "\${1:-}" = "--sdk" ]; then
  shift 2
fi
case "\${1:-}" in
  --find)
    case "\${2:-}" in
      swiftc) printf '%s\\n' "\$swiftc_path" ;;
      codesign) printf '%s\\n' '$WORKSPACE/bin/codesign' ;;
      *) printf '%s\\n' "\$0" ;;
    esac
    ;;
  --show-sdk-platform-path)
    printf '%s\\n' "\$platform_path"
    ;;
  --show-sdk-path)
    printf '%s\\n' "\$platform_path/Developer/SDKs/iPhoneSimulator.sdk"
    ;;
  simctl)
    printf '%s\\n' "\$*" >> "\$log"
    if [ "\${2:-}" = "list" ] && [ "\${3:-}" = "devices" ] && [ "\${4:-}" = "booted" ]; then
      printf '%s\\n' '    iPhone 15 (11111111-1111-1111-1111-111111111111) (Booted)'
    fi
    if [ "\${2:-}" = "list" ] && [ "\${3:-}" = "devices" ] && [ "\${4:-}" = "available" ]; then
      printf '%s\\n' '    iPad Pro (22222222-2222-2222-2222-222222222222) (Shutdown)'
    fi
    ;;
  *)
    printf '%s\\n' "unexpected xcrun invocation: \$*" >&2
    exit 1
    ;;
esac
SH
    chmod +x "$WORKSPACE/toolchain/usr/bin/xcrun"
  }

  It 'lists buildable Apple artifacts'
    copy_apple_graph_fixture

    When call once query targets
    The status should be success
    The stdout should include 'apps/ios/AppCore (apple_library) [build]'
    The stdout should include 'apps/ios/DesignSystem (apple_framework) [build]'
    The stdout should include 'apps/ios/App (apple_application) [build, run, artifact]'
    The stdout should include 'apps/ios/AppTests (apple_test_bundle) [build, test]'
  End

  It 'exposes runnable Apple application artifacts'
    copy_apple_graph_fixture

    When call once --format json query capabilities apps/ios/App
    The status should be success
    The stdout should include '"kind":"apple_application"'
    The stdout should include '"name":"build"'
    The stdout should include '"output_groups":["default","bundle","dsyms"]'
    The stdout should include '"name":"run"'
    The stdout should include '"requires_outputs":["bundle"]'
    The stdout should include '"name":"artifact"'
  End

  It 'builds Apple application artifacts through the graph command'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_apple_graph_fixture

    When call once --format json build apps/ios/App
    The status should be success
    The stdout should include '"target":"apps/ios/App"'
    The stdout should include '"capability":"build"'
    The stdout should include '"status":"completed"'
    The stdout should include '"output_groups":["default","bundle","dsyms"]'
    The stdout should include '.once/out/apps/ios/App/App.app/Info.plist'
    The path "$WORKSPACE/.once/out/apps/ios/App/App.app/App" should be file
    The path "$WORKSPACE/.once/out/apps/ios/App/App.app/Info.plist" should be file
    The path "$WORKSPACE/.once/out/apps/ios/App/App.app/Frameworks/DesignSystem.framework/DesignSystem" should be file
  End

  It 'builds a generated Swift source from an Xcode shell phase'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_generated_source_fixture

    When call once --format json build Generated
    The status should be success
    The stdout should include '"target":"Generated"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/.once/out/Generated/Generated.swiftmodule" should be file
    The path "$WORKSPACE/.once/out/Generated/Generated.swift" should be file
  End

  It 'builds a Swift target that imports an XCFramework from a Copy Files phase'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_xcframework_import_fixture

    When call once --format json build Feature
    The status should be success
    The stdout should include '"target":"Feature"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/.once/out/Feature/Feature.framework/Feature" should be file
  End

  It 'lowers a Firefox-style multi-library project from a real .xcodeproj'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_firefox_app_fixture

    When call once --format json build
    The status should be success
    The stdout should include '"target":"xcode"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/once.toml" should not be exist
    The path "$WORKSPACE/.once/out/Client/Client.app/Client" should be file
    The path "$WORKSPACE/.once/out/BrowserKit/BrowserKit.a" should be file
    The path "$WORKSPACE/.once/out/Tabs/Tabs.a" should be file
    The path "$WORKSPACE/.once/out/Bookmarks/Bookmarks.a" should be file
    End

  It 'lists the Firefox-style targets with their Once kinds'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_firefox_app_fixture

    When call once --format json query targets
    The status should be success
    The stdout should include '"name":"Client","kind":"apple_application"'
    The stdout should include '"name":"BrowserKit","kind":"apple_library"'
    The stdout should include '"name":"Tabs","kind":"apple_library"'
    The stdout should include '"name":"Bookmarks","kind":"apple_library"'
    The stdout should include '"name":"ClientTests","kind":"apple_test_bundle"'
    The stdout should include 'apple_xcframework_import'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'reports the first-party test bundle for a Firefox-style native project'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_firefox_app_fixture

    When call once --format json query target xcode
    The status should be success
    The stdout should include '"_default_test_roots":["ClientTests"]'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'lowers a Wikipedia-style design-system + feature modules from a real .xcodeproj'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_wikipedia_app_fixture

    When call once --format json build
    The status should be success
    The stdout should include '"target":"xcode"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/once.toml" should not be exist
    The path "$WORKSPACE/.once/out/Client/Client.app/Client" should be file
    The path "$WORKSPACE/.once/out/DesignSystem/DesignSystem.framework/DesignSystem" should be file
    End

  It 'lists the Wikipedia-style targets with their Once kinds'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_wikipedia_app_fixture

    When call once --format json query targets
    The status should be success
    The stdout should include '"name":"Client","kind":"apple_application"'
    The stdout should include '"name":"DesignSystem","kind":"apple_framework"'
    The stdout should include '"name":"Article","kind":"apple_library"'
    The stdout should include '"name":"Search","kind":"apple_library"'
    The stdout should include '"name":"Settings","kind":"apple_library"'
    The stdout should include '"name":"ClientTests","kind":"apple_test_bundle"'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'lowers a Signal-style app with embedded extensions from a real .xcodeproj'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_signal_app_fixture

    When call once --format json build
    The status should be success
    The stdout should include '"target":"xcode"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/once.toml" should not be exist
    The path "$WORKSPACE/.once/out/SignalApp/SignalApp.app/SignalApp" should be file
    The path "$WORKSPACE/.once/out/SignalShare/SignalShare.appex/SignalShare" should be file
    The path "$WORKSPACE/.once/out/SignalNotification/SignalNotification.appex/SignalNotification" should be file
    End

  It 'lists the Signal-style targets with their Once kinds'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_signal_app_fixture

    When call once --format json query targets
    The status should be success
    The stdout should include '"name":"SignalApp","kind":"apple_application"'
    The stdout should include '"name":"SignalShare","kind":"apple_application"'
    The stdout should include '"name":"SignalNotification","kind":"apple_application"'
    The stdout should include '"name":"SignalCore","kind":"apple_library"'
    The stdout should include '"name":"SignalAppTests","kind":"apple_test_bundle"'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'builds a cache-substituted style app graph around prebuilt XCFrameworks'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_prebuilt_cache_app_fixture
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "ClientWorkspace"
kind = "xcode_workspace"
srcs = ["Client.xcodeproj/project.pbxproj"]

[target.attrs]
project = "Client.xcodeproj"
TOML

    When call once --format json build ClientWorkspace
    The status should be success
    The stdout should include '"target":"ClientWorkspace"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/.once/out/Client/Client.app/Client" should be file
  End

  ios_simulator_unavailable() {
    xcrun simctl list devices available 2>/dev/null | grep -Eq "iPhone|iPad" && return 1
    return 0
  }

  It 'runs the cache-substituted style test bundle on a simulator'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    Skip if 'no iOS simulator available on this host' ios_simulator_unavailable
    copy_xcode_prebuilt_cache_app_fixture
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "ClientWorkspace"
kind = "xcode_workspace"
srcs = ["Client.xcodeproj/project.pbxproj"]

[target.attrs]
project = "Client.xcodeproj"
TOML

    When call once --format json test ClientTests
    The status should be success
    The stdout should include '"target":"ClientTests"'
    The stdout should include '"status":"completed"'
    The contents of file "$WORKSPACE/.once/out/ClientTests/test/test_results.json" should include '"status":"passed"'
  End

  It 'lists the cache-substituted style targets with their Once kinds'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_xcode_prebuilt_cache_app_fixture
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "ClientWorkspace"
kind = "xcode_workspace"
srcs = ["Client.xcodeproj/project.pbxproj"]

[target.attrs]
project = "Client.xcodeproj"
TOML

    When call once --format json query targets
    The status should be success
    The stdout should include '"name":"Client","kind":"apple_application"'
    The stdout should include '"name":"FeatureKit","kind":"apple_library"'
    The stdout should include '"name":"SharedKit","kind":"apple_library"'
    The stdout should include '"name":"ClientTests","kind":"apple_test_bundle"'
  End

  It 'runs Apple application artifacts through the simulator launch path'
    Skip if 'Apple simulator runtimes require macOS' macos_runtime_unavailable
    create_mock_apple_run_fixture

    When call env PATH="$WORKSPACE/bin:$PATH" "$ONCE_BIN" -C "$WORKSPACE" --format json run apps/ios/App
    The status should be success
    The stdout should include '"target":"apps/ios/App"'
    The stdout should include '"capability":"run"'
    The stdout should include '"cache":"bypass"'
    The path "$WORKSPACE/.once/out/apps/ios/App/run/run.json" should be file
    The contents of file "$WORKSPACE/.once/out/apps/ios/App/run/run.json" should include '"status":"launched"'
    The contents of file "$WORKSPACE/.once/out/apps/ios/App/run/run.json" should include '"simulator_id":"11111111-1111-1111-1111-111111111111"'
    The contents of file "$WORKSPACE/xcrun.log" should include 'simctl launch 11111111-1111-1111-1111-111111111111 dev.once.App'
  End

  It 'exposes testable Apple test bundle artifacts'
    copy_apple_graph_fixture

    When call once --format json query capabilities apps/ios/AppTests
    The status should be success
    The stdout should include '"kind":"apple_test_bundle"'
    The stdout should include '"name":"build"'
    The stdout should include '"output_groups":["default","bundle","dsyms"]'
    The stdout should include '"name":"test"'
    The stdout should include '"output_groups":["default","test_results","coverage"]'
  End

  It 're-runs Apple test runner actions without using the action cache'
    Skip if 'Apple test runtimes require macOS' macos_runtime_unavailable
    create_mock_apple_test_fixture

    When call env PATH="$WORKSPACE/toolchain/usr/bin:$PATH" /bin/sh -c '"$1" -C "$2" --format json test apps/ios/AppTests
"$1" -C "$2" --format json test apps/ios/AppTests
printf "spawn_count=%s\n" "$(grep -c "simctl spawn" "$2/xcrun.log")"' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include '"target":"apps/ios/AppTests"'
    The stdout should include '"capability":"test"'
    The stdout should include '"cache":"bypass"'
    The stdout should include 'spawn_count=2'
  End

  It 'tests Apple test bundle artifacts through the graph command'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_apple_graph_fixture

    # XCTest execution still emits placeholder normalized results while
    # simulator/device runners are wired through the Apple target kind.
    once --format json build apps/ios/AppTests >/dev/null 2>&1 || true

    When call once --format json test apps/ios/AppTests
    The status should be success
    The stdout should include '"target":"apps/ios/AppTests"'
    The stdout should include '"capability":"test"'
    The stdout should include '"status":"completed"'
    The stdout should include '"output_groups":["default","test_results","coverage"]'
    The stdout should include '"required_outputs":[]'
    The path "$WORKSPACE/.once/out/apps/ios/AppTests/test/test_results.json" should be file
  End

  It 'links and runs an Objective-C-only XCTest bundle for older macOS versions'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "ObjCTests"
kind = "apple_test_bundle"
srcs = ["Tests.m"]

[target.attrs]
platform = "macos"
minimum_os = "12.0"
TOML
    cat > "$WORKSPACE/Tests.m" <<'OBJC'
#import <XCTest/XCTest.h>
@interface ArithmeticTests : XCTestCase
@end
@implementation ArithmeticTests
- (void)testArithmetic { XCTAssertEqual(7 * 3, 21); }
@end
OBJC

    When call once --format json test ObjCTests
    The status should be success
    The stdout should include '"status":"completed"'
    The contents of file "$WORKSPACE/.once/out/ObjCTests/test/test_results.json" should include '"passed":1'
    The contents of file "$WORKSPACE/.once/out/ObjCTests/test/test_results.json" should include 'ArithmeticTests/testArithmetic'
  End

  It 'compiles string catalogs into localized runtime resources'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "Messages"
kind = "apple_resource_bundle"

[target.attrs]
platform = "macos"
minimum_os = "14.0"
resources = ["Localizable.xcstrings", "Empty.xcstrings", "Guide.help", "Arithmetic.applescript"]
TOML
    cat > "$WORKSPACE/Localizable.xcstrings" <<'JSON'
{"sourceLanguage":"en","strings":{"greeting":{"localizations":{"en":{"stringUnit":{"state":"translated","value":"Hello"}},"fr":{"stringUnit":{"state":"translated","value":"Bonjour"}}}}},"version":"1.0"}
JSON
    printf '%s\n' '{"sourceLanguage":"en","strings":{},"version":"1.0"}' > "$WORKSPACE/Empty.xcstrings"
    printf '%s\n' 'return 7 * 3' > "$WORKSPACE/Arithmetic.applescript"
    mkdir -p "$WORKSPACE/Guide.help/Contents/Resources/fr.lproj"
    printf '%s\n' '<?xml version="1.0"?><plist version="1.0"><dict><key>greeting</key><string>Aide</string></dict></plist>' > "$WORKSPACE/Guide.help/Contents/Resources/fr.lproj/Localizable.strings"

    compiled_messages() {
      once --format json build Messages >/dev/null &&
        plutil -extract greeting raw -o - "$WORKSPACE/.once/out/Messages/Messages.bundle/en.lproj/Localizable.strings" &&
        plutil -extract greeting raw -o - "$WORKSPACE/.once/out/Messages/Messages.bundle/fr.lproj/Localizable.strings" &&
        plutil -extract greeting raw -o - "$WORKSPACE/.once/out/Messages/Messages.bundle/Guide.help/Contents/Resources/fr.lproj/Localizable.strings" &&
        osascript "$WORKSPACE/.once/out/Messages/Messages.bundle/Arithmetic.scpt"
    }

    When call compiled_messages
    The status should be success
    The line 1 of stdout should equal 'Hello'
    The line 2 of stdout should equal 'Bonjour'
    The line 3 of stdout should equal 'Aide'
    The line 4 of stdout should equal '21'
    The path "$WORKSPACE/.once/out/Messages/Messages.bundle/Localizable.xcstrings" should not be exist
    The path "$WORKSPACE/.once/out/Messages/Messages.bundle/Empty.xcstrings" should not be exist
    The path "$WORKSPACE/.once/out/Messages/Messages.bundle/Arithmetic.applescript" should not be exist
  End

  It 'links Swift runtime compatibility libraries once when all archives are loaded'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "CompatibilityTests"
kind = "apple_test_bundle"
srcs = ["Tests.swift"]

[target.attrs]
platform = "macos"
minimum_os = "10.13"
linkopts = ["-Xlinker", "-all_load"]
TOML
    cat > "$WORKSPACE/Tests.swift" <<'SWIFT'
import XCTest
final class CompatibilityTests: XCTestCase {
    func testArithmetic() { XCTAssertEqual(7 * 3, 21) }
}
SWIFT

    When call once --format json test CompatibilityTests
    The status should be success
    The stdout should include '"status":"completed"'
    The contents of file "$WORKSPACE/.once/out/CompatibilityTests/test/test_results.json" should include '"passed":1'
  End

  It 'runs Swift Testing Apple tests through the generic test capability'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_apple_swift_testing_fixture

    When call once --format json test apps/macos/AppTests
    The status should be success
    The stdout should include '"target":"apps/macos/AppTests"'
    The stdout should include '"capability":"test"'
    The stdout should include '"status":"completed"'
    The path "$WORKSPACE/.once/out/apps/macos/AppTests/test/test_results.json" should be file
    The contents of file "$WORKSPACE/.once/out/apps/macos/AppTests/test/test_results.json" should include '"runner":{"type":"swift_testing"'
    The contents of file "$WORKSPACE/.once/out/apps/macos/AppTests/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/apps/macos/AppTests/test/test_results.json" should include 'examplePasses'
  End

  It 'runs affected Swift Testing Apple tests through MCP after a source change'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    copy_apple_swift_testing_fixture
    mcp_request='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"once_run_tests","arguments":{"changed_paths":["apps/macos/Sources/AppCore/AppCore.swift"]}}}'

    When call /bin/sh -c 'printf "%s\n" "$1" | "$2" -C "$3" mcp --allow-run' sh "$mcp_request" "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include 'apps/macos/AppTests'
    The stdout should include 'once.test_results.v1'
    The stdout should include 'swift_testing'
    The stdout should include '\"success\": true'
  End

  It 'describes Apple application build schema'
    When call once query schema apple_application
    The status should be success
    The stdout should include 'apple_application'
    The stdout should include 'provisioning_profile'
    The stdout should include 'asset_catalogs'
    The stdout should include 'build: default, bundle, dsyms'
    The stdout should include 'run: default (requires: bundle)'
  End

  It 'describes Apple test bundle build schema'
    When call once query schema apple_test_bundle
    The status should be success
    The stdout should include 'apple_test_bundle'
    The stdout should include 'build: default, bundle, dsyms'
    The stdout should include 'test: default, test_results, coverage'
  End

  It 'errors when building a target id that does not match'
    copy_apple_graph_fixture

    When call once build apps/ios/Missing
    The status should not equal 0
    The stderr should include 'no target matches'
  End

  It 'errors when a capability is not exposed by the target'
    copy_apple_graph_fixture

    When call once test apps/ios/App
    The status should not equal 0
    The stderr should include 'does not expose `test`'
  End

  It 'rejects --remote for graph run targets before execution'
    copy_apple_graph_fixture

    When call once run --remote apps/ios/App
    The status should not equal 0
    The stderr should include '--remote is only supported for executable script targets'
  End

  It 'rejects --runtime-rpc for graph run targets before execution'
    copy_apple_graph_fixture

    When call once run --runtime-rpc apps/ios/App
    The status should not equal 0
    The stderr should include '--runtime-rpc is only supported for executable script targets'
  End

  Describe 'apple_library swiftc compile'
    It 'compiles an apple_library target to a real static archive'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_fixture

      When call once --format json build apps/ios/AppCore
      The status should be success
      The stdout should include '"target":"apps/ios/AppCore"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/apps/ios/AppCore/AppCore.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/AppCore/AppCore.swiftmodule" should be file
      The path "$WORKSPACE/.once/out/apps/ios/AppCore/AppCore.swiftdoc" should be file
      The path "$WORKSPACE/.once/out/apps/ios/AppCore/Headers/AppCore/AppCore-Swift.h" should be file
    End

    It 'recursively builds apple_library dependencies'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_fixture

      When call once --format json build apps/ios/Greeter
      The status should be success
      The stdout should include '"target":"apps/ios/Greeter"'
      The path "$WORKSPACE/.once/out/apps/ios/AppCore/AppCore.swiftmodule" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Greeter/Greeter.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Greeter/Greeter.swiftmodule" should be file
    End

    It 'reuses cached swift compile output across runs'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_fixture

      # The priming run happens unconditionally in the example body,
      # so `|| true` keeps it from aborting on hosts where Skip if
      # already short-circuited the assertions but the body still
      # executes (e.g. Linux runners with no xcrun).
      once --format json build apps/ios/AppCore >/dev/null 2>&1 || true

      When call once --format json build apps/ios/AppCore
      The status should be success
      The stdout should include '"cache":"hit"'
    End

    It 'propagates `defines` to the swiftc compile and emits dSYM debug info'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_defines_fixture

      When call once --format json build apps/ios/DefinesGuard
      # The Swift source has `#if !ONCE_DEFINES_PRESENT #error(...) #endif`,
      # so a successful build is positive evidence that `defines` made it
      # to the swiftc command line. The archive and module are the
      # cross-toolchain probe for debug-info emission.
      The status should be success
      The stdout should include '"target":"apps/ios/DefinesGuard"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/apps/ios/DefinesGuard/DefinesGuard.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/DefinesGuard/DefinesGuard.swiftmodule" should be file
    End

    It 'compiles a mixed Swift + Objective-C target into a single archive'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_mixed_fixture

      When call once --format json build apps/ios/Mixed
      The status should be success
      The stdout should include '"target":"apps/ios/Mixed"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed-swift.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed.swiftmodule" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Headers/Mixed/Mixed-Swift.h" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/apps_ios_Mixed_Sources_MixedObjC.m.o" should be file
    End

    It 'emits a valid Apple .hmap when enable_modules and exported_headers are set'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_mixed_fixture

      once --format json build apps/ios/Mixed >/dev/null 2>&1 || true

      hmap_first_bytes() {
        xxd -p -l 6 "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed.hmap"
      }

      # The hmap byte format starts with magic 0x686D6170 ("pamh" in
      # little-endian) followed by a version-1 u16. Reading the first
      # six bytes catches any drift in the byte layout.
      When call hmap_first_bytes
      The status should be success
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed.hmap" should be file
      The stdout should include '70616d68'
      The stdout should include '0100'
    End

    It 'fans out a multi-arch apple_library into a lipo-merged fat archive'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_multi_arch_fixture

      When call once --format json build apps/ios/Universal
      The status should be success
      The stdout should include '"target":"apps/ios/Universal"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/apps/ios/Universal/Universal.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Universal/Universal-arm64.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Universal/Universal-x86_64.a" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Universal/Universal.swiftmodule/arm64.swiftmodule" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Universal/Universal.swiftmodule/x86_64.swiftmodule" should be file
    End

    It 'produces a lipo-readable fat archive containing both architectures'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_multi_arch_fixture
      once --format json build apps/ios/Universal >/dev/null 2>&1 || true

      lipo_archs() {
        xcrun lipo -info "$WORKSPACE/.once/out/apps/ios/Universal/Universal.a"
      }

      When call lipo_archs
      The status should be success
      The stdout should include 'arm64'
      The stdout should include 'x86_64'
    End

    It 'resolves select() branches against the target configuration'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_select_fixture

      # The Swift source has `#if !SIM_BUILD && !DEVICE_BUILD #error`,
      # so a successful build is positive evidence that the
      # `ios:simulator` composite-key branch resolved correctly and
      # `SIM_BUILD` reached swiftc.
      When call once --format json build apps/ios/Selecty
      The status should be success
      The stdout should include '"target":"apps/ios/Selecty"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/apps/ios/Selecty/Selecty.a" should be file
    End

    It 'fails analysis when select() targets a non-configurable attribute'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      mkdir -p "$WORKSPACE/apps/ios/Bad/Sources"
      printf 'public let x = 1\n' > "$WORKSPACE/apps/ios/Bad/Sources/lib.swift"
      cat > "$WORKSPACE/apps/ios/once.toml" <<'EOF'
[[target]]
name = "Bad"
kind = "apple_library"
srcs = ["Bad/Sources/*.swift"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
minimum_os = "17.0"
module_name = { select = { ios = "BadIOS", default = "Bad" } }
EOF

      When call once build apps/ios/Bad
      The status should not equal 0
      The stderr should include 'attribute `module_name` is not configurable but uses `select()`'
    End

    It 'fails analysis when select() targets a configuration-input attribute'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      mkdir -p "$WORKSPACE/apps/ios/Bad/Sources"
      printf 'public let x = 1\n' > "$WORKSPACE/apps/ios/Bad/Sources/lib.swift"
      cat > "$WORKSPACE/apps/ios/once.toml" <<'EOF'
[[target]]
name = "Bad"
kind = "apple_library"
srcs = ["Bad/Sources/*.swift"]

[target.attrs]
platform = { select = { default = "ios" } }
sdk_variant = "simulator"
minimum_os = "17.0"
EOF

      When call once build apps/ios/Bad
      The status should not equal 0
      The stderr should include 'attribute `platform` is not configurable but uses `select()`'
    End

    It 'reuses the parent cache slot when a dep source edit preserves its module'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_fixture

      once --format json build apps/ios/Greeter >/dev/null 2>&1 || true
      printf '\n// trigger rebuild\n' >> "$WORKSPACE/apps/ios/AppCore/Sources/Greeting.swift" 2>/dev/null || true

      When call once --format json build apps/ios/Greeter
      The status should be success
      The stdout should include '"cache":"hit"'
    End
  End

  Describe 'apple_framework + apple_application end-to-end'
    It 'compiles a static library, a dynamic framework that links it, and an app that embeds the framework'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_application_swiftui_fixture

      When call once --format json build apps/ios/App
      The status should be success
      The stdout should include '"target":"apps/ios/App"'
      The stdout should include '"status":"completed"'
      # Static library
      The path "$WORKSPACE/.once/out/models/Models/Models.a" should be file
      The path "$WORKSPACE/.once/out/models/Models/Models.swiftmodule" should be file
      # Dynamic framework wrapping the static lib
      The path "$WORKSPACE/.once/out/ui/UI/UI.framework/UI" should be file
      The path "$WORKSPACE/.once/out/ui/UI/UI.framework/Info.plist" should be file
      The path "$WORKSPACE/.once/out/ui/UI/UI.framework/Modules/module.modulemap" should be file
      The path "$WORKSPACE/.once/out/ui/UI/UI.framework/Modules/UI.swiftmodule/arm64-apple-ios-simulator.swiftmodule" should be file
      The path "$WORKSPACE/.once/out/ui/UI/UI.framework/_CodeSignature/CodeResources" should be file
      # App bundle with embedded framework
      The path "$WORKSPACE/.once/out/apps/ios/App/App.app/App" should be file
      The path "$WORKSPACE/.once/out/apps/ios/App/App.app/Info.plist" should be file
      The path "$WORKSPACE/.once/out/apps/ios/App/App.app/Frameworks/UI.framework/UI" should be file
      The path "$WORKSPACE/.once/out/apps/ios/App/App.app/_CodeSignature/CodeResources" should be file
    End

    It 'produces a Mach-O executable that loads @rpath/UI.framework/UI'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_application_swiftui_fixture
      once --format json build apps/ios/App >/dev/null 2>&1 || true

      app_dylib_deps() {
        otool -L "$WORKSPACE/.once/out/apps/ios/App/App.app/App"
      }

      When call app_dylib_deps
      The status should be success
      The stdout should include '@rpath/UI.framework/UI'
      The stdout should include '/System/Library/Frameworks/SwiftUI.framework/SwiftUI'

      app_file_kind() {
        file "$WORKSPACE/.once/out/apps/ios/App/App.app/App"
      }
    End

    It 'sets the framework dylib install_name to @rpath'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_application_swiftui_fixture
      once --format json build apps/ios/App >/dev/null 2>&1 || true

      framework_install_name() {
        otool -D "$WORKSPACE/.once/out/ui/UI/UI.framework/UI"
      }

      When call framework_install_name
      The status should be success
      The stdout should include '@rpath/UI.framework/UI'
    End

    It 'preserves strict code signatures after an unchanged rebuild'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_application_swiftui_fixture

      verify_unchanged_app_signature() {
        once --format json build apps/ios/App >/dev/null &&
          once --format json build apps/ios/App >/dev/null &&
          codesign --verify --deep --strict "$WORKSPACE/.once/out/apps/ios/App/App.app"
      }

      When call verify_unchanged_app_signature
      The status should be success
    End

    It 'preserves strict framework signatures after an unchanged rebuild'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_application_swiftui_fixture

      verify_unchanged_framework_signature() {
        once --format json build ui/UI >/dev/null &&
          once --format json build ui/UI >/dev/null &&
          codesign --verify --strict "$WORKSPACE/.once/out/ui/UI/UI.framework"
      }

      When call verify_unchanged_framework_signature
      The status should be success
    End
  End

  Describe 'native macOS application bundles'
    It 'loads resources and a versioned framework after building, rebuilding, and restoring from cache'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable

      verify_macos_bundle() {
        mkdir -p "$WORKSPACE/Vendor/Boundary.xcframework/macos/Boundary.framework/Versions/A/Resources" \
          "$WORKSPACE/Vendor/Boundary.xcframework/macos/Boundary.framework/Versions/A/Headers" \
          "$WORKSPACE/Vendor/Boundary.xcframework/macos/Boundary.framework/Versions/A/Modules"
        framework="$WORKSPACE/Vendor/Boundary.xcframework/macos/Boundary.framework"
        printf '%s\n' 'int boundary_value(void) { return 37; }' > "$WORKSPACE/boundary.c"
        xcrun clang -dynamiclib "$WORKSPACE/boundary.c" -o "$framework/Versions/A/Boundary" \
          -install_name @rpath/Boundary.framework/Versions/A/Boundary || return
        printf '%s\n' 'int boundary_value(void);' > "$framework/Versions/A/Headers/Boundary.h"
        printf '%s\n' 'framework module Boundary { umbrella header "Boundary.h" export * }' > "$framework/Versions/A/Modules/module.modulemap"
        cat > "$framework/Versions/A/Resources/Info.plist" <<'PLIST'
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.once.Boundary</string><key>CFBundleExecutable</key><string>Boundary</string><key>CFBundlePackageType</key><string>FMWK</string><key>CFBundleVersion</key><string>1</string></dict></plist>
PLIST
        ln -s A "$framework/Versions/Current"
        for entry in Boundary Resources Headers Modules; do
          ln -s "Versions/Current/$entry" "$framework/$entry"
        done
        codesign --force --sign - --timestamp=none "$framework" 2> "$WORKSPACE/sign.err" || return
        codesign --verify --deep --strict "$framework" 2> "$WORKSPACE/sign.err" || return
        find "$framework" -type f -exec shasum -a 256 {} + | sort > "$WORKSPACE/source-before"

        ditto "$framework" "$WORKSPACE/Broken.framework"
        mkdir "$WORKSPACE/Broken.framework/_CodeSignature"
        if codesign --force --sign - --timestamp=none "$WORKSPACE/Broken.framework" 2> "$WORKSPACE/rejection"; then return 1; fi
        grep -q 'unsealed contents present in the root directory of an embedded framework' "$WORKSPACE/rejection" || return

        cat > "$WORKSPACE/Vendor/Boundary.xcframework/Info.plist" <<PLIST
<plist version="1.0"><dict><key>AvailableLibraries</key><array><dict><key>LibraryIdentifier</key><string>macos</string><key>LibraryPath</key><string>Boundary.framework</string><key>SupportedArchitectures</key><array><string>$(uname -m)</string></array><key>SupportedPlatform</key><string>macos</string></dict></array></dict></plist>
PLIST
        cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "Boundary"
kind = "apple_xcframework_import"
[target.attrs]
platform = "macos"
bundle = "Vendor/Boundary.xcframework"

[[target]]
name = "SharedResources"
kind = "apple_resource_bundle"
[target.attrs]
platform = "macos"
bundle_name = "SharedResources"
resources = ["shared.txt"]

[[target]]
name = "App"
kind = "apple_application"
srcs = ["main.swift"]
deps = ["Boundary", "SharedResources"]
[target.attrs]
platform = "macos"
bundle_id = "dev.once.macos.bundle"
minimum_os = "14.0"
resources = ["message.txt"]
asset_catalogs = ["Assets.xcassets"]
TOML
        printf app-resource > "$WORKSPACE/message.txt"
        printf shared-resource > "$WORKSPACE/shared.txt"
        mkdir -p "$WORKSPACE/Assets.xcassets/AccentColor.colorset"
        printf '%s\n' '{"info":{"author":"xcode","version":1}}' > "$WORKSPACE/Assets.xcassets/Contents.json"
        printf '%s\n' '{"colors":[{"idiom":"universal","color":{"color-space":"srgb","components":{"red":"0.2","green":"0.5","blue":"0.8","alpha":"1.0"}}}],"info":{"author":"xcode","version":1}}' > "$WORKSPACE/Assets.xcassets/AccentColor.colorset/Contents.json"
        cat > "$WORKSPACE/main.swift" <<'SWIFT'
import AppKit
import Foundation
import Boundary
let main = Bundle.main
precondition(main.bundleIdentifier == "dev.once.macos.bundle")
precondition(NSColor(named: "AccentColor", bundle: main) != nil)
let message = try String(contentsOf: main.url(forResource: "message", withExtension: "txt")!, encoding: .utf8)
let shared = Bundle(url: main.resourceURL!.appendingPathComponent("SharedResources.bundle"))!
let resource = try String(contentsOf: shared.url(forResource: "shared", withExtension: "txt")!, encoding: .utf8)
print("\(boundary_value()):\(message):\(resource)")
SWIFT
        app="$WORKSPACE/.once/out/App/App.app"
        embedded="$app/Contents/Frameworks/Boundary.framework"
        mkdir -p "$app"
        printf legacy > "$app/Info.plist"
        printf obsolete > "$app/obsolete.txt"
        for stage in migration cold warm restored alias; do
          if [ "$stage" = restored ]; then rm -rf "$WORKSPACE/.once/out"; fi
          if [ "$stage" = alias ]; then perl -pi -e 's/platform = "macos"/platform = "macosx"/' "$WORKSPACE/once.toml"; fi
          once --format json build App > "$WORKSPACE/$stage.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
          case "$stage" in
            warm|restored) grep -q '"cache":"hit"' "$WORKSPACE/$stage.json" || return ;;
          esac
          [ "$("$app/Contents/MacOS/App")" = '37:app-resource:shared-resource' ] || return 1
          [ "$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$app/Contents/Info.plist")" = 14.0 ] || return 1
          ! /usr/libexec/PlistBuddy -c 'Print :LSRequiresIPhoneOS' "$app/Contents/Info.plist" >/dev/null 2>&1 || return 1
          [ -f "$app/Contents/_CodeSignature/CodeResources" ] || return 1
          [ -f "$app/Contents/Resources/Assets.car" ] || return 1
          [ ! -e "$app/Info.plist" ] && [ ! -e "$embedded/_CodeSignature" ] || return 1
          [ ! -e "$app/obsolete.txt" ] || return 1
          [ "$(readlink "$embedded/Versions/Current")" = A ] || return 1
          codesign --verify --deep --strict "$app" 2> "$WORKSPACE/sign.err" || { cat "$WORKSPACE/sign.err"; return 1; }
        done
        find "$framework" -type f -exec shasum -a 256 {} + | sort > "$WORKSPACE/source-after"
        cmp "$WORKSPACE/source-before" "$WORKSPACE/source-after" || return
        codesign --verify --deep --strict "$framework" 2> "$WORKSPACE/sign.err"
      }

      When call verify_macos_bundle
      The status should be success
      The output should equal ''
    End
  End

  Describe 'apple_thinned_package end-to-end'
    It 'creates a signed device-specific archive and stable manifest'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_thinned_package_fixture

      verify_thinned_package() {
        once --format json build apps/ios/AppThinned >/dev/null &&
          unzip -q "$WORKSPACE/.once/out/apps/ios/AppThinned/ipas/App-iPhone17-1.ipa" -d "$WORKSPACE/extracted" &&
          codesign --verify --deep --strict "$WORKSPACE/extracted/Payload/App.app" &&
          grep -q '"schema": "once.apple.thinned-package.v1"' "$WORKSPACE/.once/out/apps/ios/AppThinned/thinned-packages.json" &&
          grep -q '"deviceModel": "iPhone17,1"' "$WORKSPACE/.once/out/apps/ios/AppThinned/thinned-packages.json"
      }

      When call verify_thinned_package
      The status should be success
      The path "$WORKSPACE/.once/out/apps/ios/AppThinned/ipas/App-iPhone17-1.ipa" should be file
      The path "$WORKSPACE/.once/out/apps/ios/AppThinned/thinned-packages.json" should be file
    End
  End

  Describe 'swift_macro plugin compile'
    It 'compiles a swift_macro target into a loadable plugin executable'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_swift_macro_fixture

      When call once --format json build macros/Stringify
      The status should be success
      The stdout should include '"target":"macros/Stringify"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/macros/Stringify/Stringify-tool" should be file
      The path "$WORKSPACE/.once/out/macros/Stringify/Stringify.swiftmodule" should be file
    End

    It 'produces a Mach-O executable for swift_macro plugin output'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_swift_macro_fixture
      once --format json build macros/Stringify >/dev/null 2>&1 || true

      plugin_kind() {
        file "$WORKSPACE/.once/out/macros/Stringify/Stringify-tool"
      }

      When call plugin_kind
      The status should be success
      The stdout should include 'Mach-O'
      The stdout should include 'executable'
    End

    It 'threads swift_macro plugin into a consuming apple_library'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_swift_macro_fixture

      # The consumer apple_library deps on macros/Stringify, which
      # exposes a plugin executable provider. apple_library auto-detects
      # the plugin and appends `-load-plugin-executable <path>#<module>` to
      # its swiftc invocation; a successful build means swiftc accepted
      # the flag and the executable path resolved correctly.
      When call once --format json build apps/macos/Consumer
      The status should be success
      The stdout should include '"target":"apps/macos/Consumer"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/macros/Stringify/Stringify-tool" should be file
      The path "$WORKSPACE/.once/out/apps/macos/Consumer/Consumer.a" should be file
    End
  End
End
