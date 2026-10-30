#shellcheck shell=bash

Describe 'Swift package resource catalogs'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  package_asset_toolchain_unavailable() {
    [ "$(uname -s)" != Darwin ] || ! xcrun --find actool >/dev/null 2>&1
  }

  check_package_resources() {
    local output line
    output="$("$app/Contents/MacOS/ResourceProbe" 2> "$WORKSPACE/runtime.err")" || return
    while IFS= read -r line || [ -n "$line" ]; do
      if [ "$line" != "sandbox_extension_issue_file_to_process failed for $app: 1 (Operation not permitted)" ]; then
        cat "$WORKSPACE/runtime.err" >&2
        return 1
      fi
    done < "$WORKSPACE/runtime.err"
    [ "$output" = 'package resources loaded' ]
  }

  exercise_package_resources() {
    cp -R "$REPO_ROOT/fixtures/swift_package_resources/." "$WORKSPACE/"
    ONCE_CHANGE_TRACKER=0 once build SwiftPackage_ResourceFixtures_ResourceProbe --format json > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    app="$WORKSPACE/.once/out/SwiftPackage_ResourceFixtures_ResourceProbe/ResourceProbe.app"
    check_package_resources || return 1
    [ "$(find "$app" -name Assets.car | wc -l | tr -d ' ')" = 2 ] || return 1
    mv "$WORKSPACE/.once/out" "$SPEC_ROOT/saved-outputs"
    ONCE_CHANGE_TRACKER=0 once build SwiftPackage_ResourceFixtures_ResourceProbe --format json > "$WORKSPACE/restored.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    check_package_resources
  }

  It 'loads compiled colors and copied resources through Bundle.module after a build and cache restore'
    Skip if 'requires Xcode' package_asset_toolchain_unavailable
    When call exercise_package_resources
    The status should be success
    The output should equal ''
  End
End
