#shellcheck shell=bash

Describe 'swift compatibility'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  install_system_swift() {
    mkdir -p "$WORKSPACE/tools"
    cat > "$WORKSPACE/tools/swift" <<'SH'
#!/bin/sh
printf '%s\n' "$*"
exit "${SWIFT_EXIT_CODE:-0}"
SH
    chmod +x "$WORKSPACE/tools/swift"
  }

  swift_package_tools_unavailable() {
    [ "$(uname -s)" != "Darwin" ] || ! command -v swift >/dev/null 2>&1 || ! command -v xcrun >/dev/null 2>&1
  }

  copy_native_package() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/swift-package-workspace-native-project/." "$WORKSPACE/"
  }

  test_manifest_build_settings() {
    mkdir -p "$WORKSPACE/Sources/Native/include" "$WORKSPACE/Sources/Probe"
    cat > "$WORKSPACE/Package.swift" <<'SWIFT'
// swift-tools-version: 6.2
import Foundation
import PackageDescription
let environment = ProcessInfo.processInfo.environment
let package = Package(name: "Probe", products: [.executable(name: "Probe", targets: ["Probe"])], targets: [
    .target(name: "Native", cSettings: [.treatAllWarnings(as: .error)],
            cxxSettings: [.treatAllWarnings(as: .warning)],
            linkerSettings: [.linkedLibrary("sqlite3"), .linkedLibrary("z"), .linkedLibrary("c++"),
                             .linkedFramework("Security"), .linkedFramework("Foundation")]),
    .executableTarget(name: "Probe", dependencies: ["Native"], swiftSettings: [
        .define(environment["ONCE_PROBE_VALUE"] == "second" ? "SECOND" : "FIRST"),
        .treatAllWarnings(as: environment["ONCE_PROBE_ALLOW_WARNING"] == "1" ? .warning : .error),
    ]),
])
SWIFT
    cat > "$WORKSPACE/Sources/Native/include/Native.h" <<'C'
#ifdef __cplusplus
extern "C" {
#endif
int nativeValue(void);
#ifdef __cplusplus
}
#endif
C
    cat > "$WORKSPACE/Sources/Native/value.cpp" <<'CXX'
#include "Native.h"
#include <sqlite3.h>
#include <zlib.h>
#include <string>
#warning C++ warning must remain nonfatal despite the C warning policy
int nativeValue(void) {
    return sqlite3_libversion_number() > 0 && std::string(zlibVersion()).size() > 0 ? 23 : 0;
}
CXX
    cat > "$WORKSPACE/Sources/Probe/main.swift" <<'SWIFT'
import Native
#warning("Swift warning policy probe")
#if SECOND
print("second:\(nativeValue())")
#else
print("first:\(nativeValue())")
#endif
SWIFT
    if ONCE_PROBE_ALLOW_WARNING=0 once --memory-limit 2GiB build --quiet > "$WORKSPACE/fatal.out" 2> "$WORKSPACE/fatal.err"; then
      return 1
    fi
    grep -q 'error: Swift warning policy probe' "$WORKSPACE/fatal.err" || return
    ONCE_PROBE_ALLOW_WARNING=1 ONCE_PROBE_VALUE=first once --memory-limit 2GiB build --quiet > "$WORKSPACE/first.out" 2> "$WORKSPACE/first.err" || {
      cat "$WORKSPACE/first.err"
      return 1
    }
    binary="$WORKSPACE/.once/out/SwiftPackage_Probe_Probe/Probe.app/Contents/MacOS/Probe"
    [ "$("$binary")" = first:23 ] || return 1
    ONCE_PROBE_ALLOW_WARNING=1 ONCE_PROBE_VALUE=second once --memory-limit 2GiB build --quiet > "$WORKSPACE/second.out" 2> "$WORKSPACE/second.err" || {
      cat "$WORKSPACE/second.err"
      return 1
    }
    [ "$("$binary")" = second:23 ]
  }

  test_dependency_resources() {
    mkdir -p "$WORKSPACE/Sources/Data" "$WORKSPACE/Tests/DataTests" \
      "$WORKSPACE/Sources/CValue/include" "$WORKSPACE/Sources/CValue/Private/nested"
    cat > "$WORKSPACE/Package.swift" <<'SWIFT'
// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "ResourcePackage", targets: [
    .target(name: "CValue", cSettings: [.headerSearchPath("Private")]),
    .target(name: "Data", dependencies: ["CValue"], resources: [.copy("value.txt")]),
    .testTarget(name: "DataTests", dependencies: ["Data"]),
])
SWIFT
    printf 'resource payload\n' > "$WORKSPACE/Sources/Data/value.txt"
    printf 'int resourceNumber(void);\n' > "$WORKSPACE/Sources/CValue/include/CValue.h"
    printf '#define RESOURCE_NUMBER 17\n' > "$WORKSPACE/Sources/CValue/Private/nested/value.h"
    cat > "$WORKSPACE/Sources/CValue/value.c" <<'C'
#include "CValue.h"
#include "nested/value.h"
int resourceNumber(void) { return RESOURCE_NUMBER; }
C
    cat > "$WORKSPACE/Sources/Data/Data.swift" <<'SWIFT'
import Foundation
import CValue
public func resourceValue() throws -> String {
    try String(contentsOf: Bundle.module.url(forResource: "value", withExtension: "txt")!, encoding: .utf8)
}
public func headerValue() -> Int32 { resourceNumber() }
SWIFT
    cat > "$WORKSPACE/Tests/DataTests/DataTests.swift" <<'SWIFT'
import XCTest
import Data
final class DataTests: XCTestCase {
    func testResource() throws {
        XCTAssertEqual(try resourceValue(), "resource payload\n")
        XCTAssertEqual(headerValue(), 17)
    }
}
SWIFT
    once --memory-limit 2GiB --cpu-limit 2 test --quiet > "$WORKSPACE/test.out" 2> "$WORKSPACE/test.err" || {
      cat "$WORKSPACE/test.out" "$WORKSPACE/test.err"
      return 1
    }
    jq -e '.summary.total == 1 and .summary.passed == 1 and .cases[0].status == "passed"' \
      "$WORKSPACE/.once/out/SwiftPackage_ResourcePackage_DataTests/test/test_results.json" >/dev/null
  }

  clear_swift_build_outputs() {
    find "$WORKSPACE/.once/out" -type f -delete
    find "$WORKSPACE/.once/out" -depth -type d -exec rmdir {} \;
  }

  swift_library_evidence() {
    "$ONCE_BIN" -C "$WORKSPACE" --format json query evidence SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage:build
  }

  swift_consumer_evidence() {
    "$ONCE_BIN" -C "$WORKSPACE" --format json query evidence SwiftPackage_OnceNativeSwiftPackage_OnceNativeConsumer:build
  }

  swift_dependency_evidence() {
    "$ONCE_BIN" -C "$WORKSPACE" --format json query evidence SwiftPackage_OnceNativeSwiftPackage_OnceNativeDependency:build
  }

  verify_swift_cache_evidence() {
    cmp "$WORKSPACE/first-artifacts" "$WORKSPACE/second-artifacts" || return
    jq -e --slurpfile first "$WORKSPACE/first-evidence.json" '
      ([.[] | select(.cache == "hit") | .action_digest] | sort) ==
      ([$first[0][] | .action_digest] | sort)
    ' "$WORKSPACE/second-evidence.json" >/dev/null || return
    jq -e '
      sort_by(.created_at_unix_ms) | .[-4:] |
      ([.[] | select(.cache == "hit")] | length == 2) and
      ([.[] | select(.cache == "miss")] | length == 2) and
      any(.[]; .cache == "miss" and (.outputs | keys[] | endswith(".a"))) and
      any(.[]; .cache == "miss" and (.outputs | keys[] | endswith(".swiftmodule")))
    ' "$WORKSPACE/third-evidence.json" >/dev/null
  }

  verify_consumer_reuses_unchanged_module() {
    jq -e \
      --slurpfile before "$WORKSPACE/dependency-before-evidence.json" \
      --slurpfile after "$WORKSPACE/dependency-after-evidence.json" '
      def latest_module(records):
        [records[] | select(.outputs | has(".once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeDependency/OnceNativeDependency.swiftmodule"))]
        | sort_by(.created_at_unix_ms)
        | last
        | .outputs[".once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeDependency/OnceNativeDependency.swiftmodule"];
      (latest_module($before[0]) == latest_module($after[0])) and
      ($after[0] | sort_by(.created_at_unix_ms) | .[-4:] | ([.[] | select(.cache == "miss")] | length == 2)) and
      (sort_by(.created_at_unix_ms) | .[-2:] | length == 2 and all(.[]; .cache == "hit"))
    ' "$WORKSPACE/consumer-evidence.json" >/dev/null
  }

  restore_swift_artifacts_from_cache() {
    copy_native_package

    once swift -- build >/dev/null 2>&1
    find "$WORKSPACE/.once/out" -type f -print | sort > "$WORKSPACE/first-artifacts"
    swift_library_evidence > "$WORKSPACE/first-evidence.json"
    clear_swift_build_outputs

    once swift -- build >/dev/null 2>&1
    find "$WORKSPACE/.once/out" -type f -print | sort > "$WORKSPACE/second-artifacts"
    swift_library_evidence > "$WORKSPACE/second-evidence.json"

    printf '\npublic func cacheProbe() {}\n' >> "$WORKSPACE/Sources/OnceNativeSwiftPackage/Greeting.swift"
    clear_swift_build_outputs
    once swift -- build >/dev/null 2>&1
    swift_library_evidence > "$WORKSPACE/third-evidence.json"

    verify_swift_cache_evidence
  }

  reuse_consumer_when_module_is_unchanged() {
    copy_native_package

    once swift -- build >/dev/null 2>&1
    swift_dependency_evidence > "$WORKSPACE/dependency-before-evidence.json"
    perl -0pi -e 's/"one"/"two"/' "$WORKSPACE/Sources/OnceNativeDependency/Dependency.swift"
    clear_swift_build_outputs
    once swift -- build >/dev/null 2>&1
    swift_dependency_evidence > "$WORKSPACE/dependency-after-evidence.json"
    swift_consumer_evidence > "$WORKSPACE/consumer-evidence.json"

    verify_consumer_reuses_unchanged_module
  }

  It 'passes unsupported invocations through without a Once trailer'
    install_system_swift

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" swift -- package resolve
    The status should be success
    The stdout should equal 'package resolve'
    The stderr should not include 'cache '
  End

  It 'preserves the system swift exit status for unsupported invocations'
    install_system_swift

    When call env SWIFT_EXIT_CODE=17 PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" swift -- build -c release
    The status should be failure
    The status should equal 17
    The stdout should equal 'build -c release'
  End

  It 'routes a compatible debug build through Once'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable
    copy_native_package

    When call once swift -- build
    The status should be success
    The stdout should include 'once: build swift_package (swift_package_workspace)'
    The stderr should include 'once  build swift_package'
    The stderr should include 'Done'
  End

  It 'routes a compatible debug test through Once'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable
    copy_native_package

    When call once swift -- test
    The status should be success
    The stdout should include 'once: test SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests (apple_test_bundle)'
    The stderr should include 'once  test SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests'
    The stderr should include 'Done'
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests/test/test_results.json" should be file
  End

  It 'loads dependency resources from a signed macOS test bundle'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable

    When call test_dependency_resources
    The status should be success
    The output should equal ''
    The path "$WORKSPACE/.once/out/SwiftPackage_ResourcePackage_DataTests/DataTests.xctest/Contents/Resources/ResourcePackage_Data.bundle/value.txt" should be file
  End

  It 'honors package linking and warning policies and invalidates inherited manifest switches'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable

    When call test_manifest_build_settings
    The status should be success
    The output should equal ''
  End

  It 'builds and tests a discovered Swift package without a Once manifest'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable
    copy_native_package
    rm "$WORKSPACE/once.toml"

    When call /bin/sh -c '"$1" -C "$2" build --quiet && "$1" -C "$2" test --quiet' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include 'once: build swift_package (swift_package_workspace)'
    The stdout should include 'test batches'
    The path "$WORKSPACE/once.toml" should not be exist
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests/test/test_results.json" should be file
  End

  It 'restores every declared artifact and invalidates only changed Swift actions'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable

    When call restore_swift_artifacts_from_cache
    The status should be success
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage/OnceNativeSwiftPackage.a" should be file
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage/OnceNativeSwiftPackage.abi.json" should be file
  End

  It 'reuses a consumer when a dependency implementation preserves its module'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable

    When call reuse_consumer_when_module_is_unchanged
    The status should be success
  End
End
