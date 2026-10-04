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
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Development")" = 16 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/SwiftPackage_VaporGraph_VaporTests" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/SwiftPackage_VaporGraph_XCTVapor" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/SwiftPackage_swift-nio_NIOTransportServices" ] || return 1
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    graph_check_action_hits || return

    mv "$WORKSPACE/.once/out" "$WORKSPACE/saved-outputs"
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Development")" = 16 ] || return 1

    perl -pi -e 's/GRAPH_VALUE 7/GRAPH_VALUE 9/' "$WORKSPACE/Sources/CVaporBcrypt/include/CVaporBcrypt.h"
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Development")" = 18 ] || return 1

    perl -pi -e 's/logValue = 2/logValue = 3/' "$WORKSPACE/Dependencies/swift-log/Sources/Logging/Logging.swift"
    graph_build SwiftPackage_VaporGraph_Development || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/SwiftPackage_VaporGraph_Development/Development.app/Development")" = 21 ] || return 1
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
    [ "$("$WORKSPACE/.once/out/NetNewsWire/NetNewsWire.app/NetNewsWire")" = 11 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/NetNewsWire-iOS" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/XcodePackage_xcode_ios_Account_Account" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/NetNewsWireTests" ] || return 1
    graph_build NetNewsWire || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    graph_check_action_hits || return

    perl -pi -e 's/CORE_VALUE 5/CORE_VALUE 8/' "$WORKSPACE/Modules/RSCore/Sources/RSCoreObjC/include/RSCoreObjC.h"
    graph_build NetNewsWire || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/NetNewsWire/NetNewsWire.app/NetNewsWire")" = 17 ] || return 1

    perl -pi -e 's/GRAPH_MAC/GRAPH_CHANGED/' "$WORKSPACE/Config/macOS.xcconfig"
    graph_snapshot || return
    jq -e '.NetNewsWire.attrs.swift_flags | index("GRAPH_CHANGED") != null and index("GRAPH_MAC") == null' "$WORKSPACE/graph.json" >/dev/null || return
    perl -pi -e 's/fileRef = MAIN_FILE/fileRef = UNUSED_FILE/' "$WORKSPACE/NetNewsWire.xcodeproj/project.pbxproj"
    graph_snapshot || return
    jq -e '.NetNewsWire.srcs == ["Sources/NotAMember.swift"] and ."NetNewsWire-iOS".srcs == ["Sources/NotAMember.swift"]' "$WORKSPACE/graph.json" >/dev/null || return
    graph_validate
  }

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
