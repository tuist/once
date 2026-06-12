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

  create_apple_workspace() {
    mkdir -p "$WORKSPACE/apps/ios/Sources/AppCore/include" "$WORKSPACE/apps/ios/Sources/DesignSystem" "$WORKSPACE/apps/ios/Sources/App" "$WORKSPACE/apps/ios/Sources/AppTests"
    cat > "$WORKSPACE/apps/ios/Sources/AppCore/include/AppCore.h" <<'EOF'
#pragma once
EOF
    cat > "$WORKSPACE/apps/ios/Sources/AppCore/AppCore.swift" <<'EOF'
public struct AppCore {
    public static let name = "AppCore"
}
EOF
    cat > "$WORKSPACE/apps/ios/Sources/DesignSystem/DesignSystem.swift" <<'EOF'
import AppCore
public struct DesignSystem {
    public static let owner = AppCore.name
}
EOF
    cat > "$WORKSPACE/apps/ios/Sources/App/App.swift" <<'EOF'
import DesignSystem

@main
struct AppEntry {
    static let owner = DesignSystem.owner
    static func main() {}
}
EOF
    cat > "$WORKSPACE/apps/ios/Sources/AppTests/AppTests.swift" <<'EOF'
public struct AppTests {
    public static let name = "AppTests"
}
EOF

    cat > "$WORKSPACE/apps/ios/once.toml" <<'EOF'
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/AppCore/**/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
headers = ["Sources/AppCore/include/AppCore.h"]
exported_headers = ["Sources/AppCore/include/AppCore.h"]
sdk_frameworks = ["Foundation"]
swift_flags = ["-warnings-as-errors"]

[[target]]
name = "DesignSystem"
kind = "apple_framework"
srcs = ["Sources/DesignSystem/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
bundle_id = "dev.once.DesignSystem"
minimum_os = "17.0"

[[target]]
name = "App"
kind = "apple_application"
srcs = ["Sources/App/**/*.swift"]
deps = ["./DesignSystem"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
bundle_id = "dev.once.App"
minimum_os = "17.0"
families = ["iphone", "ipad"]

[[target]]
name = "AppTests"
kind = "apple_test_bundle"
srcs = ["Sources/AppTests/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
minimum_os = "17.0"
EOF
  }

  It 'lists buildable Apple artifacts'
    create_apple_workspace

    When call once query targets
    The status should be success
    The stdout should include 'apps/ios/AppCore (apple_library) [build]'
    The stdout should include 'apps/ios/DesignSystem (apple_framework) [build]'
    The stdout should include 'apps/ios/App (apple_application) [build, run]'
    The stdout should include 'apps/ios/AppTests (apple_test_bundle) [build, test]'
  End

  It 'exposes runnable Apple application artifacts'
    create_apple_workspace

    When call once --format json query capabilities apps/ios/App
    The status should be success
    The stdout should include '"kind":"apple_application"'
    The stdout should include '"name":"build"'
    The stdout should include '"output_groups":["default","bundle","dsyms"]'
    The stdout should include '"name":"run"'
    The stdout should include '"requires_outputs":["bundle"]'
  End

  It 'builds Apple application artifacts through the graph command'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    create_apple_workspace

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

  It 'runs Apple application artifacts through the graph command'
    create_apple_workspace

    When call once --format json run apps/ios/App
    The status should be success
    The stdout should include '"target":"apps/ios/App"'
    The stdout should include '"capability":"run"'
    The stdout should include '"status":"completed"'
    The stdout should include '"required_outputs":["bundle"]'
    The path "$WORKSPACE/.once/out/apps/ios/App/run/run.json" should be file
  End

  It 'exposes testable Apple test bundle artifacts'
    create_apple_workspace

    When call once --format json query capabilities apps/ios/AppTests
    The status should be success
    The stdout should include '"kind":"apple_test_bundle"'
    The stdout should include '"name":"build"'
    The stdout should include '"output_groups":["default","bundle","dsyms"]'
    The stdout should include '"name":"test"'
    The stdout should include '"output_groups":["default","test_results","coverage"]'
    The stdout should include '"requires_outputs":["bundle"]'
  End

  It 'tests Apple test bundle artifacts through the graph command'
    Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
    create_apple_workspace

    # `once test` first builds the bundle through the Starlark impl,
    # then runs the test capability which is still served by the
    # CLI's placeholder runner (it writes `test_results.json` and
    # `coverage.json` without invoking xctest itself).
    once --format json build apps/ios/AppTests >/dev/null 2>&1 || true

    When call once --format json test apps/ios/AppTests
    The status should be success
    The stdout should include '"target":"apps/ios/AppTests"'
    The stdout should include '"capability":"test"'
    The stdout should include '"status":"completed"'
    The stdout should include '"output_groups":["default","test_results","coverage"]'
    The stdout should include '"required_outputs":["bundle"]'
    The path "$WORKSPACE/.once/out/apps/ios/AppTests/test/test_results.json" should be file
  End

  It 'describes Apple application build and run schemas'
    When call once query schema apple_application
    The status should be success
    The stdout should include 'apple_application'
    The stdout should include 'provisioning_profile'
    The stdout should include 'asset_catalogs'
    The stdout should include 'run: default'
  End

  It 'describes Apple test bundle build and test schemas'
    When call once query schema apple_test_bundle
    The status should be success
    The stdout should include 'apple_test_bundle'
    The stdout should include 'test: default, test_results, coverage'
  End

  It 'errors when building a target id that does not match'
    create_apple_workspace

    When call once build apps/ios/Missing
    The status should not equal 0
    The stderr should include 'no target matches'
  End

  It 'errors when a capability is not exposed by the target'
    create_apple_workspace

    When call once test apps/ios/App
    The status should not equal 0
    The stderr should include 'does not expose `test`'
  End

  It 'rejects --remote for graph run targets'
    create_apple_workspace

    When call once run --remote apps/ios/App
    The status should not equal 0
    The stderr should include '--remote is only supported for executable script targets'
  End

  It 'rejects --runtime-rpc for graph run targets'
    create_apple_workspace

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
      The path "$WORKSPACE/.once/out/apps/ios/AppCore/AppCore-Swift.h" should be file
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
      # to the swiftc command line. `emit_dsym` adds `-g`, which manifests
      # as a populated `.swiftsourceinfo` alongside the archive; we assert
      # the artifacts exist as the easiest cross-toolchain probe.
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
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed-Swift.h" should be file
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/apps_ios_Mixed_Sources_MixedObjC.m.o" should be file
    End

    It 'emits a valid Apple .hmap when enable_modules and exported_headers are set'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_mixed_fixture

      once --format json build apps/ios/Mixed >/dev/null 2>&1 || true

      hmap_first_bytes() {
        xxd -p -l 6 "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed.hmap"
      }

      # The hmap byte format starts with magic 0x68616D70 ("pmah" in
      # little-endian) followed by a version-1 u16. Reading the first
      # six bytes catches any drift in the byte layout.
      When call hmap_first_bytes
      The status should be success
      The path "$WORKSPACE/.once/out/apps/ios/Mixed/Mixed.hmap" should be file
      The stdout should include '706d6168'
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
      The stderr should include 'attribute `module_name` is not configurable but uses select()'
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
      The stderr should include 'attribute `platform` cannot use select()'
    End

    It 'invalidates the parent cache slot when a dep source changes'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_apple_library_fixture

      once --format json build apps/ios/Greeter >/dev/null 2>&1 || true
      printf '\n// trigger rebuild\n' >> "$WORKSPACE/apps/ios/AppCore/Sources/Greeting.swift" 2>/dev/null || true

      When call once --format json build apps/ios/Greeter
      The status should be success
      The stdout should include '"cache":"miss"'
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
      The path "$WORKSPACE/.once/out/ui/UI/UI.framework/Modules/UI.swiftmodule" should be file
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
  End

  Describe 'swift_macro plugin compile'
    It 'compiles a swift_macro target into a loadable plugin dylib'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_swift_macro_fixture

      When call once --format json build macros/Stringify
      The status should be success
      The stdout should include '"target":"macros/Stringify"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/macros/Stringify/libStringify.dylib" should be file
      The path "$WORKSPACE/.once/out/macros/Stringify/Stringify.swiftmodule" should be file
    End

    It 'produces a Mach-O dylib for swift_macro plugin output'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_swift_macro_fixture
      once --format json build macros/Stringify >/dev/null 2>&1 || true

      dylib_kind() {
        file "$WORKSPACE/.once/out/macros/Stringify/libStringify.dylib"
      }

      When call dylib_kind
      The status should be success
      The stdout should include 'Mach-O'
      The stdout should include 'dynamically linked shared library'
    End

    It 'threads swift_macro plugin into a consuming apple_library'
      Skip if 'apple toolchain unavailable on this host' apple_toolchain_unavailable
      copy_swift_macro_fixture

      # The consumer apple_library deps on macros/Stringify, which
      # exposes a plugin_dylib provider. apple_library auto-detects
      # the plugin and appends `-load-plugin-library <dylib>` to its
      # swiftc invocation; a successful build means swiftc accepted
      # the flag and the dylib path resolved correctly.
      When call once --format json build apps/macos/Consumer
      The status should be success
      The stdout should include '"target":"apps/macos/Consumer"'
      The stdout should include '"status":"completed"'
      The path "$WORKSPACE/.once/out/macros/Stringify/libStringify.dylib" should be file
      The path "$WORKSPACE/.once/out/apps/macos/Consumer/Consumer.a" should be file
    End
  End
End
