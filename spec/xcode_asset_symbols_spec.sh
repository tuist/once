#shellcheck shell=bash

Describe 'Xcode asset symbol setting'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  asset_toolchain_unavailable() {
    [ "$(uname -s)" != Darwin ] || ! xcrun --find actool >/dev/null 2>&1
  }

  check_asset_color() {
    local output line
    output="$("$app/Contents/MacOS/Assets" 2> "$WORKSPACE/runtime.err")" || return
    while IFS= read -r line || [ -n "$line" ]; do
      if [ "$line" != "sandbox_extension_issue_file_to_process failed for $app: 1 (Operation not permitted)" ]; then
        cat "$WORKSPACE/runtime.err" >&2
        return 1
      fi
    done < "$WORKSPACE/runtime.err"
    [ "$output" = 'asset color loaded' ]
  }

  exercise_asset_symbols() {
    cp -R "$REPO_ROOT/fixtures/xcode_asset_symbols/." "$WORKSPACE/"
    app="$WORKSPACE/.once/out/Assets/Assets.app"
    ONCE_CHANGE_TRACKER=0 once build Assets --format json > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ -s "$WORKSPACE/.once/out/Assets/Assets.app/Contents/Resources/Assets.car" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/Assets/GeneratedAssetSymbols.swift" ] || return 1
    check_asset_color || return 1
    sed -i '' 's/ASSETCATALOG_COMPILER_GENERATE_ASSET_SYMBOLS = NO/ASSETCATALOG_COMPILER_GENERATE_ASSET_SYMBOLS = YES/' "$WORKSPACE/Assets.xcodeproj/project.pbxproj"
    sed -i '' '/extension NSColor {/,/^}/d' "$WORKSPACE/main.swift"
    ONCE_CHANGE_TRACKER=0 once build Assets --format json > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ -s "$WORKSPACE/.once/out/Assets/GeneratedAssetSymbols.swift" ] || return 1
    check_asset_color || return 1
  }

  It 'disables conflicting accessors without losing runtime assets, and enables them when requested'
    Skip if 'requires Xcode' asset_toolchain_unavailable
    When call exercise_asset_symbols
    The status should be success
    The output should equal ''
  End
End
