#shellcheck shell=bash

Describe 'real-world native graph shapes'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  native_graph_toolchain_unavailable() {
    [ "$(uname -s)" != Darwin ] || ! xcrun --find swiftc >/dev/null 2>&1
  }

  graph_once() {
    ONCE_CHANGE_TRACKER=0 RUST_LOG=off once --memory-limit 2GiB "$@" --format json
  }

  graph_snapshot() {
    graph_once query 'MATCH (t:Target) RETURN t.id, t.kind, t.attrs, t.deps, t.srcs' > "$WORKSPACE/query.json" || return
    jq '.rows | map({key: .[0], value: {kind: .[1], attrs: .[2], deps: .[3], srcs: .[4]}}) | from_entries' "$WORKSPACE/query.json" > "$WORKSPACE/graph.json" || return
    jq -e 'keys as $ids | all(.[]; all(.deps[]; . as $dep | $ids | index($dep) != null))' "$WORKSPACE/graph.json" >/dev/null
  }

  graph_build() {
    graph_once build "$1" > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || {
      cat "$WORKSPACE/build.err"
      return 1
    }
  }

  graph_validate() {
    graph_once query validate-workspace > "$WORKSPACE/validation.json" || return
    jq -e '.valid and (.diagnostics | length == 0)' "$WORKSPACE/validation.json" >/dev/null || {
      cat "$WORKSPACE/validation.json"
      return 1
    }
  }

  graph_check_action_hits() {
    graph_once query evidence > "$WORKSPACE/evidence-warm.json" || return
    jq -e --slurpfile cold "$WORKSPACE/evidence-cold.json" '
      ($cold[0] | map(.id)) as $old |
      [.[] | select(.id as $id | $old | index($id) == null)] as $new |
      ($new | length) == ($cold[0] | length) and
      ($new | all(.cache == "hit")) and
      ([$new[] | {action_digest, input_digest, outputs}] | sort_by(.action_digest)) ==
      ([$cold[0][] | {action_digest, input_digest, outputs}] | sort_by(.action_digest))
    ' "$WORKSPACE/evidence-warm.json" >/dev/null
  }

  exercise_vapor_graph() {
    cp "$REPO_ROOT/fixtures/native_graphs/vapor/Package.swift" "$WORKSPACE/"
    cp -R "$REPO_ROOT/fixtures/native_graphs/vapor/Sources" "$REPO_ROOT/fixtures/native_graphs/vapor/Tests" "$REPO_ROOT/fixtures/native_graphs/vapor/Dependencies" "$WORKSPACE/"
    graph_snapshot || return
    jq -e -f "$REPO_ROOT/spec/support/native_graphs/vapor.jq" "$WORKSPACE/graph.json" >/dev/null || return
    [ ! -e "$WORKSPACE/Package.resolved" ] || return 1
    graph_validate || return

    graph_build SwiftPackage_VaporGraph_Development || return
    graph_once query evidence > "$WORKSPACE/evidence-cold.json" || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Contents/MacOS/Development")" = 16 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/SwiftPackage_VaporGraph_VaporTests" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/SwiftPackage_VaporGraph_XCTVapor" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/SwiftPackage_swift-nio_NIOTransportServices" ] || return 1
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    graph_check_action_hits || return

    mv "$WORKSPACE/.once/out" "$WORKSPACE/saved-outputs"
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Contents/MacOS/Development")" = 16 ] || return 1

    perl -pi -e 's/GRAPH_VALUE 7/GRAPH_VALUE 9/' "$WORKSPACE/Sources/CVaporBcrypt/include/CVaporBcrypt.h"
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Contents/MacOS/Development")" = 18 ] || return 1

    perl -pi -e 's/logValue = 2/logValue = 3/' "$WORKSPACE/Dependencies/swift-log/Sources/Logging/Logging.swift"
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Contents/MacOS/Development")" = 21 ] || return 1
  }

  exercise_netnewswire_graph() {
    cp -R "$REPO_ROOT/fixtures/native_graphs/netnewswire/NetNewsWire.xcodeproj" "$REPO_ROOT/fixtures/native_graphs/netnewswire/Config" "$REPO_ROOT/fixtures/native_graphs/netnewswire/Sources" "$REPO_ROOT/fixtures/native_graphs/netnewswire/Extensions" "$REPO_ROOT/fixtures/native_graphs/netnewswire/Tests" "$REPO_ROOT/fixtures/native_graphs/netnewswire/Modules" "$WORKSPACE/"
    cp -R "$REPO_ROOT/fixtures/native_graphs/netnewswire/Vendor" "$WORKSPACE/"
    sh "$WORKSPACE/Vendor/build.sh" || return
    graph_snapshot || return
    jq -e -f "$REPO_ROOT/spec/support/native_graphs/netnewswire.jq" "$WORKSPACE/graph.json" >/dev/null || return
    graph_validate || return

    graph_build NetNewsWire || return
    graph_once query evidence > "$WORKSPACE/evidence-cold.json" || return
    [ "$("$WORKSPACE/.once/out/NetNewsWire/NetNewsWire.app/Contents/MacOS/NetNewsWire")" = 11 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/NetNewsWire-iOS" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/XcodePackage_xcode_ios_Account_Account" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/NetNewsWireTests" ] || return 1
    graph_build NetNewsWire || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    graph_check_action_hits || return

    perl -pi -e 's/CORE_VALUE 5/CORE_VALUE 8/' "$WORKSPACE/Modules/RSCore/Sources/RSCoreObjC/include/RSCoreObjC.h"
    graph_build NetNewsWire || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/NetNewsWire/NetNewsWire.app/Contents/MacOS/NetNewsWire")" = 17 ] || return 1

    perl -pi -e 's/GRAPH_MAC/GRAPH_CHANGED/' "$WORKSPACE/Config/macOS.xcconfig"
    graph_snapshot || return
    jq -e '.NetNewsWire.attrs.swift_flags | index("GRAPH_CHANGED") != null and index("GRAPH_MAC") == null' "$WORKSPACE/graph.json" >/dev/null || return
    perl -pi -e 's/fileRef = MAIN_FILE/fileRef = UNUSED_FILE/' "$WORKSPACE/NetNewsWire.xcodeproj/project.pbxproj"
    graph_snapshot || return
    jq -e '.NetNewsWire.srcs == ["Sources/NotAMember.swift"] and ."NetNewsWire-iOS".srcs == ["Sources/NotAMember.swift"]' "$WORKSPACE/graph.json" >/dev/null || return
    graph_validate
  }

  exercise_xcode_build_tools() {
    cp -R "$REPO_ROOT/fixtures/xcode_workspace_compatibility/." "$WORKSPACE/"
    origin="$SPEC_ROOT/GeneratorSupport"
    mv "$WORKSPACE/App/Packages/GeneratorSupport" "$origin"
    git -C "$origin" init -q || return
    git -C "$origin" config user.name Once || return
    git -C "$origin" config user.email once@example.invalid || return
    git -C "$origin" add . || return
    git -C "$origin" commit -qm pinned || return
    revision="$(git -C "$origin" rev-parse HEAD)"
    git -C "$origin" tag 1.0.0 || return
    perl -pi -e 's/Value = 40/Value = 90/' "$origin/Sources/GeneratorSupport/Value.swift"
    git -C "$origin" commit -qam newer || return
    git -C "$origin" tag 1.1.0 || return
    sed "s|path: \"../GeneratorSupport\"|url: \"file://$origin\", from: \"1.0.0\"|" "$WORKSPACE/App/Packages/Generated/Package.swift" > "$WORKSPACE/manifest.tmp"
    mv "$WORKSPACE/manifest.tmp" "$WORKSPACE/App/Packages/Generated/Package.swift"
    mkdir -p "$WORKSPACE/App/Fetch.xcworkspace/xcshareddata/swiftpm"
    jq -n --arg revision "$revision" --arg url "file://$origin" '{version: 2, pins: [{identity: "generatorsupport", kind: "remoteSourceControl", location: $url, state: {revision: $revision, version: "1.0.0"}}]}' > "$WORKSPACE/App/Fetch.xcworkspace/xcshareddata/swiftpm/Package.resolved"
    graph_snapshot 2> "$WORKSPACE/query.err" || return
    jq -e '[.[] | select(.kind == "swift_package_build_tools") | .attrs.build_role] | sort == ["destination", "destination", "tools", "tools"]' "$WORKSPACE/graph.json" >/dev/null || return
    [ ! -d "$WORKSPACE/.once/out" ] || return 1
    [ "$(git -C "$WORKSPACE/.once/xcode-packages/generatorsupport" rev-parse HEAD)" = "$revision" ] || return 1
    graph_validate 2> "$WORKSPACE/validation.err" || return

    ONCE_CHANGE_TRACKER=0 once --memory-limit 2GiB xcodebuild -- -workspace App/Fetch.xcworkspace -scheme Fetch -configuration Development -quiet build > "$WORKSPACE/build.out" 2> "$WORKSPACE/build.err" || {
      cat "$WORKSPACE/build.out" "$WORKSPACE/build.err"
      return 1
    }
    binary="$WORKSPACE/.once/out/App/Fetch/Fetch.app/Contents/MacOS/Fetch"
    [ "$("$binary")" = '42:payload' ] || return 1
    [ "$(find "$WORKSPACE/.once/out" -name lint.txt -exec cat {} \;)" = validated ] || return 1
    graph_build App/Fetch || return
    find "$WORKSPACE/.once/out" -name lint.txt -delete
    graph_build App/Fetch || return
    [ "$(find "$WORKSPACE/.once/out" -name lint.txt -exec cat {} \;)" = validated ] || return 1
    graph_once test App/FetchTests > "$WORKSPACE/test.json" 2> "$WORKSPACE/test.err" || {
      cat "$WORKSPACE/test.err"
      return 1
    }
    jq -e '.summary.total == 1 and .summary.passed == 1' "$WORKSPACE/.once/out/App/FetchTests/test/test_results.json" >/dev/null || return

    host_probe="$(jq -r 'to_entries[] | select(.value.kind == "swift_macro") | .key' "$WORKSPACE/graph.json")"
    graph_build "$host_probe" || return
    "$WORKSPACE/.once/out/$host_probe/HostProbe-tool" || return
    macro_test="$(jq -r 'to_entries[] | select(.value.kind == "apple_test_bundle" and .value.attrs.product_name == "HostProbeTests") | .key' "$WORKSPACE/graph.json")"
    graph_once test "$macro_test" > "$WORKSPACE/macro-test.json" 2> "$WORKSPACE/macro-test.err" || { cat "$WORKSPACE/macro-test.err"; return 1; }
    jq -e '.summary.total == 1 and .summary.passed == 1' "$WORKSPACE/.once/out/$macro_test/test/test_results.json" >/dev/null || return
    ONCE_GENERATED_CXX=1 graph_build App/Fetch || return
    [ "$("$binary")" = '42:payload' ] || return 1
    if ONCE_GENERATED_WARNING=1 graph_once build App/Fetch > "$WORKSPACE/generated-warning.json" 2> "$WORKSPACE/generated-warning.err"; then
      return 1
    fi
    grep -q 'error:.*Generated warning must be fatal' "$WORKSPACE/generated-warning.err" || return
    ONCE_HEADER_VALUE=8 graph_build App/Fetch || return
    [ "$("$binary")" = '43:payload' ] || return 1
    graph_build App/Fetch || return
    [ "$("$binary")" = '42:payload' ] || return 1

    jq -r '.[] | select(.kind == "swift_package_build_tools" and .attrs.target_name == "Generated" and .attrs.build_role == "destination") | .attrs |
      "\n[[target]]\nname = \"NoTraits\"\nkind = \"swift_package_build_tools\"\n[target.attrs]\npackage_path = \(.package_path | @json)\ntarget_name = \"Generated\"\ndeclares_traits = true\n[target.attrs.dependencies]\n" +
      (.dependencies | to_entries | map("\(.key | @json) = \(.value | @json)") | join("\n"))' "$WORKSPACE/graph.json" >> "$WORKSPACE/once.toml"
    graph_build NoTraits || return
    grep -q 'buildToolValue = 140' "$(find "$WORKSPACE/.once/out/NoTraits" -path '*/Generated/destination/*/Build.swift')" || return

    sed '/.macro(name: "HostProbe"/d; /.testTarget(name: "HostProbeTests"/d' "$WORKSPACE/App/Packages/Generated/Package.swift" > "$WORKSPACE/manifest.tmp"
    mv "$WORKSPACE/manifest.tmp" "$WORKSPACE/App/Packages/Generated/Package.swift"
    host_library="$(jq -r 'to_entries[] | select(.value.kind == "apple_library" and .value.attrs.module_name == "Generated" and (.key | endswith("_MacroHost"))) | .key' "$WORKSPACE/graph.json")"
    graph_build "$host_library" || return
    [ -s "$WORKSPACE/.once/out/$host_library/Generated.a" ] || return 1

    printf '\n#warning "Nested warning must be fatal"\n' >> "$WORKSPACE/App/Packages/Generated/Sources/CGenerated/Nested/Value.c"
    if graph_once build App/Fetch > "$WORKSPACE/warning.json" 2> "$WORKSPACE/warning.err"; then
      return 1
    fi
    grep -q 'error:.*Nested warning must be fatal' "$WORKSPACE/warning.err" || return
    sed '/#warning/d' "$WORKSPACE/App/Packages/Generated/Sources/CGenerated/Nested/Value.c" > "$WORKSPACE/source.tmp"
    mv "$WORKSPACE/source.tmp" "$WORKSPACE/App/Packages/Generated/Sources/CGenerated/Nested/Value.c"

    FETCH_TUIST_BUILD=1 graph_build App/Fetch || return
    [ "$("$binary")" = '43:payload' ] || return 1
    perl -pi -e 's/fixtureGeneratorValue \+ traitAdjustment/fixtureGeneratorValue + traitAdjustment + 1/' "$WORKSPACE/App/Packages/Generated/Sources/Generator/main.swift"
    graph_build App/Fetch || return
    [ "$("$binary")" = '43:payload' ] || return 1
    perl -pi -e 's/prebuildValue = 2/prebuildValue = 3/' "$WORKSPACE/App/Packages/Generated/Tools/prebuild.swift"
    graph_build App/Fetch || return
    [ "$("$binary")" = '44:payload' ] || return 1
    perl -pi -e 's/Development/InternalBeta/' "$WORKSPACE/App/once.toml"
    graph_build App/Fetch || return
    [ "$("$binary")" = '44:payload' ] || return 1

    rm -rf "$WORKSPACE/.once/out"
    graph_build App/Fetch || return
    [ "$("$binary")" = '44:payload' ] || return 1
    if ONCE_PLUGIN_FAIL=1 graph_once build App/Fetch > "$WORKSPACE/failure.json" 2> "$WORKSPACE/failure.err"; then
      return 1
    fi
    grep -q 'Requested lint failure' "$WORKSPACE/failure.err"
  }

  It 'builds and tests an Xcode workspace with source-built tools, prebuild discovery, resources, and output-free commands'
    Skip if 'requires Xcode' native_graph_toolchain_unavailable
    When call exercise_xcode_build_tools
    The status should be success
    The output should equal ''
  End

  It 'parses a Vapor-shaped package graph and executes only the requested dependency closure'
    Skip if 'requires an Apple Swift compiler' native_graph_toolchain_unavailable
    When call exercise_vapor_graph
    The status should be success
    The output should equal ''
  End

  It 'separates NetNewsWire-shaped destinations and tracks native membership, settings, and local dependencies'
    Skip if 'requires Xcode' native_graph_toolchain_unavailable
    When call exercise_netnewswire_graph
    The status should be success
    The output should equal ''
  End
End
