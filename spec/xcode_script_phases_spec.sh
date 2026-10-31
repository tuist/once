#shellcheck shell=bash

Describe 'native script phase execution'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  script_toolchain_unavailable() {
    [ "$(uname -s)" != Darwin ] || ! xcrun --find swiftc >/dev/null 2>&1
  }

  script_once() {
    ONCE_CHANGE_TRACKER=0 RUST_LOG=off once --memory-limit 2GiB "$@" --format json
  }

  exercise_native_scripts() {
    cp -R "$REPO_ROOT/fixtures/xcode_script_phases/." "$WORKSPACE/"
    script_once query validate-workspace > "$WORKSPACE/validation.json" || return
    jq -e '.valid and (.diagnostics | length == 0)' "$WORKSPACE/validation.json" >/dev/null || return
    script_once build Script_App > "$WORKSPACE/cold.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    jq -e '.cache == "bypass"' "$WORKSPACE/cold.json" >/dev/null || return
    script_once query evidence > "$WORKSPACE/evidence-cold.json" || return
    script_once build Script_App > "$WORKSPACE/warm.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    jq -e '.cache == "bypass"' "$WORKSPACE/warm.json" >/dev/null || return
    script_once query evidence > "$WORKSPACE/evidence-warm.json" || return
    jq -e --slurpfile cold "$WORKSPACE/evidence-cold.json" '
      ($cold[0] | map(.id)) as $old |
      [.[] | select(.id as $id | $old | index($id) == null)] as $new |
      ($new | length > 2) and
      ($new | map(select(.cache == "hit")) | length > 2) and
      ($new | all(.cache == "hit" or .cache == "bypass" or
        (.cache == "miss" and (.outputs | has(".once/out/Script_App/Script App.app/Contents/_CodeSignature/CodeResources") or has(".once/out/Script_App/Script App.app/Contents/Resources/observed.txt"))))) and
      ($new | any(.cache == "bypass" and (.outputs | has(".once/out/Script_App/Script App.app"))))
    ' "$WORKSPACE/evidence-warm.json" >/dev/null || return
    [ "$(wc -l < "$WORKSPACE/.once/out/Script_App/Intermediates/runs.txt" | tr -d ' ')" = 2 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/undeclared.txt")" = 2 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/observed.txt")" = 2 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/generated-resource.txt" ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Info.plist")" = 42 ] || return 1
    [ "$("$WORKSPACE/.once/out/Script_App/Script App.app/Contents/MacOS/Script App")" = 'script graph' ] || return 1
    codesign --verify "$WORKSPACE/.once/out/Script_App/Script App.app" 2> "$WORKSPACE/signature.err" || { cat "$WORKSPACE/signature.err"; return 1; }
    mv "$WORKSPACE/.once/out" "$WORKSPACE/saved-outputs"
    script_once build Script_App > "$WORKSPACE/restore.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Info.plist")" = 42 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/from-script.txt")" = 'native graph inputs' ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/undeclared.txt")" = 1 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/observed.txt")" = 1 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/Script_App/Script App.app/Contents/Resources/generated-resource.txt" ] || return 1
    codesign --verify "$WORKSPACE/.once/out/Script_App/Script App.app" 2> "$WORKSPACE/signature.err" || return
    mv "$WORKSPACE/.once/out" "$WORKSPACE/saved-restored-outputs"
    script_once build Script_App --config token=alternate > "$WORKSPACE/configured.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    jq -e '.outputs | any(contains("/from-script.txt"))' "$WORKSPACE/configured.json" >/dev/null
  }

  It 'maps native settings and file lists, caches declared scripts, and reruns untracked product edits before signing'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_native_scripts
    The status should be success
    The output should equal ''
  End

  build_script_free_native_fixture() {
    cp -R "$REPO_ROOT/fixtures/xcode_native_extension/." "$WORKSPACE/"
    perl -pi -e 's/buildPhases = \(ENVIRONMENT, PREPARE, HOST_SOURCES\)/buildPhases = (HOST_SOURCES)/' "$WORKSPACE/Native.xcodeproj/project.pbxproj"
    printf 'print("native" + helperValue)\n' > "$WORKSPACE/Host/main.swift"
    script_once build Host > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
  }

  exercise_partial_module_paths() {
    build_script_free_native_fixture || return
    leaked_modules="$(find "$WORKSPACE" -maxdepth 1 -name '*.swiftmodule' -print)"
    [ -z "$leaked_modules" ] || { printf 'workspace-root partial modules:\n%s\n' "$leaked_modules"; return 1; }
    [ -f "$WORKSPACE/.once/out/Host/Host.swiftmodule" ] || return 1
    [ -f "$WORKSPACE/.once/out/Widget/Widget.swiftmodule" ] || return 1
    [ "$("$WORKSPACE/.once/out/Host/Host.app/Contents/MacOS/Host")" = 'native host' ]
  }

  It 'keeps multi-source application and extension partial modules out of the workspace root'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_partial_module_paths
    The status should be success
    The output should equal ''
  End

  exercise_extension_bundle() {
    build_script_free_native_fixture || return
    extension="$WORKSPACE/.once/out/Host/Host.app/Contents/PlugIns/Widget.appex"
    [ -x "$extension/Contents/MacOS/Widget" ] || { printf 'missing embedded extension executable\n'; return 1; }
    [ ! -e "$WORKSPACE/.once/out/Widget/Widget.app" ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundlePackageType' "$extension/Contents/Info.plist")" = 'XPC!' ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$extension/Contents/Info.plist")" = dev.once.native-extension.widget ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionPointIdentifier' "$extension/Contents/Info.plist")" = com.apple.share-services ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionPrincipalClass' "$extension/Contents/Info.plist")" = Widget.ExtensionPrincipal ] || return 1
    codesign --verify --deep --strict "$WORKSPACE/.once/out/Host/Host.app" 2> "$WORKSPACE/signature.err" || { cat "$WORKSPACE/signature.err"; return 1; }
  }

  It 'embeds a native appex with extension metadata and a valid host signature without script phases'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_extension_bundle
    The status should be success
    The output should equal ''
  End

  copy_isolated_native_script_fixture() {
    cp -R "$REPO_ROOT/fixtures/xcode_native_extension/." "$WORKSPACE/"
    perl -pi -e '
      s/buildPhases = \(ENVIRONMENT, PREPARE, HOST_SOURCES\)/buildPhases = (PREPARE, HOST_SOURCES)/;
      s/, "\$\{NOT_DEFINED\}\/missing", "\$\(NOT_DEFINED\)\/missing"//;
    ' "$WORKSPACE/Native.xcodeproj/project.pbxproj"
    cat > "$WORKSPACE/prepare.sh" <<'SH'
set -eu
mkdir -p "$SCRIPT_OUTPUT_FILE_0"
cat "$SCRIPT_INPUT_FILE_0" > "$SCRIPT_OUTPUT_FILE_1"
SH
  }

  exercise_unresolved_script_paths() {
    copy_isolated_native_script_fixture
    perl -pi -e 's/inputPaths = \("\$\(SRCROOT\)\/value.txt"\)/inputPaths = ("\$(SRCROOT)\/value.txt", "prefix\/\${NOT_DEFINED}\/missing", "prefix\/\$(NOT_DEFINED)\/missing")/' "$WORKSPACE/Native.xcodeproj/project.pbxproj"
    printf '[ "$SCRIPT_INPUT_FILE_COUNT" = 1 ]\n' >> "$WORKSPACE/prepare.sh"
    script_once build Host > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$("$WORKSPACE/.once/out/Host/Host.app/Contents/MacOS/Host")" = 'native host' ]
  }

  It 'skips unresolved script input expressions in both variable syntaxes before hashing'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_unresolved_script_paths
    The status should be success
    The output should equal ''
  End

  exercise_dsym_script_settings() {
    copy_isolated_native_script_fixture
    cat >> "$WORKSPACE/prepare.sh" <<'SH'
printf '%s:%s\n' "${DWARF_DSYM_FOLDER_PATH#"$TARGET_BUILD_DIR"}" "$DWARF_DSYM_FILE_NAME" > "$SCRIPT_OUTPUT_FILE_0/dsym.txt"
SH
    script_once build Host > "$WORKSPACE/default.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$(cat "$WORKSPACE/Vendor/Plugin.app/dsym.txt")" = ':Host.app.dSYM' ] || return 1
    perl -pi -e 's/PRODUCT_NAME = Host;/PRODUCT_NAME = Host; DWARF_DSYM_FOLDER_PATH = "\$(TARGET_BUILD_DIR)\/Symbols"; DWARF_DSYM_FILE_NAME = Custom.dSYM;/' "$WORKSPACE/Native.xcodeproj/project.pbxproj"
    script_once build Host > "$WORKSPACE/override.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$(cat "$WORKSPACE/Vendor/Plugin.app/dsym.txt")" = '/Symbols:Custom.dSYM' ]
  }

  It 'provides native dSYM script defaults and honors explicit build-setting overrides'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_dsym_script_settings
    The status should be success
    The output should equal ''
  End

  exercise_directory_output_compile_cache() {
    copy_isolated_native_script_fixture
    : > "$WORKSPACE/preparation-runs.txt"
    printf 'printf "run\\n" >> "$SRCROOT/preparation-runs.txt"\n' >> "$WORKSPACE/prepare.sh"
    script_once build Host > "$WORKSPACE/cold.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    script_once query evidence > "$WORKSPACE/evidence-cold.json" || return
    script_once build Host > "$WORKSPACE/warm.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    script_once query evidence > "$WORKSPACE/evidence-warm.json" || return
    [ "$(wc -l < "$WORKSPACE/preparation-runs.txt" | tr -d ' ')" = 2 ] || return 1
    [ "$("$WORKSPACE/.once/out/Host/Host.app/Contents/MacOS/Host")" = 'native host' ] || return 1
    jq -r --slurpfile cold "$WORKSPACE/evidence-cold.json" '
      ($cold[0] | map(.id)) as $old |
      [.[] | select(.id as $id | $old | index($id) == null)] as $new |
      ($new[] | select(.cache != "bypass" and (.outputs | has(".once/out/Host/Host.swiftmodule") and has(".once/out/Host/Host.swiftdoc"))) | .cache),
      ($new[] | select((.outputs | keys) == [".once/out/Host/Host.app/Contents/MacOS/Host"]) | .cache)
    ' "$WORKSPACE/evidence-warm.json"
  }

  It 'reuses both application compiler actions after an unrelated always-run directory output'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_directory_output_compile_cache
    The status should be success
    The output should equal "$(printf 'hit\nhit')"
  End

  exercise_native_extension() {
    local ONCE_SCRIPT_CI_VALUE ONCE_DECLARED_CI_VALUE PRODUCT_NAME
    cp -R "$REPO_ROOT/fixtures/xcode_native_extension/." "$WORKSPACE/"
    export ONCE_SCRIPT_CI_VALUE='from caller'
    export ONCE_DECLARED_CI_VALUE='first caller value'
    export PRODUCT_NAME='caller must not override native setting'
    script_once query validate-workspace > "$WORKSPACE/validation.json" || return
    jq -e '.valid and (.diagnostics | length == 0)' "$WORKSPACE/validation.json" >/dev/null || return
    script_once build Host > "$WORKSPACE/cold.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$(cat "$WORKSPACE/.once/out/Host/environment.txt")" = 'first caller value|Host' ] || return 1
    script_once query evidence > "$WORKSPACE/evidence-cold.json" || return
    extension="$WORKSPACE/.once/out/Host/Host.app/Contents/PlugIns/Widget.appex"
    [ -x "$extension/Contents/MacOS/Widget" ] || return 1
    [ ! -e "$WORKSPACE/.once/out/Widget/Widget.app" ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionPointIdentifier' "$extension/Contents/Info.plist")" = com.apple.share-services ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionPrincipalClass' "$extension/Contents/Info.plist")" = Widget.ExtensionPrincipal ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$extension/Contents/Info.plist")" = dev.once.native-extension.widget ] || return 1
    [ "$("$WORKSPACE/.once/out/Host/Host.app/Contents/MacOS/Host")" = 'native host' ] || return 1
    codesign --verify --deep --strict "$WORKSPACE/.once/out/Host/Host.app" 2> "$WORKSPACE/signature.err" || { cat "$WORKSPACE/signature.err"; return 1; }
    [ -z "$(find "$WORKSPACE" -maxdepth 1 -name '*.swiftmodule' -print)" ] || return 1
    script_once build Host > "$WORKSPACE/warm.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    script_once query evidence > "$WORKSPACE/evidence-warm.json" || return
    jq -e --slurpfile cold "$WORKSPACE/evidence-cold.json" '
      ($cold[0] | map(.id)) as $old |
      [.[] | select(.id as $id | $old | index($id) == null)] as $new |
      ($new | any(.cache == "hit" and (.outputs | has(".once/out/Host/Host.swiftmodule")))) and
      ($new | any(.cache == "hit" and (.outputs | has(".once/out/Host/Host.app/Contents/MacOS/Host")))) and
      ($new | any(.cache == "hit" and (.outputs | has(".once/out/Host/environment.txt"))))
    ' "$WORKSPACE/evidence-warm.json" >/dev/null || return
    [ "$(wc -l < "$WORKSPACE/.once/out/Host/Intermediates/preparation-runs.txt" | tr -d ' ')" = 2 ] || return 1
    export ONCE_DECLARED_CI_VALUE='changed caller value'
    script_once build Host > "$WORKSPACE/environment-changed.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$(cat "$WORKSPACE/.once/out/Host/environment.txt")" = 'changed caller value|Host' ] || return 1
    script_once query evidence > "$WORKSPACE/evidence-environment-changed.json" || return
    jq -e --slurpfile warm "$WORKSPACE/evidence-warm.json" '
      ($warm[0] | map(.id)) as $old |
      [.[] | select(.id as $id | $old | index($id) == null)] |
      any(.cache == "miss" and (.outputs | has(".once/out/Host/environment.txt")))
    ' "$WORKSPACE/evidence-environment-changed.json" >/dev/null || return
    printf 'let generatedValue = "changed"\n' > "$WORKSPACE/value.txt"
    script_once build Host > "$WORKSPACE/changed.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$("$WORKSPACE/.once/out/Host/Host.app/Contents/MacOS/Host")" = 'changed host' ] || return 1
    [ "$(wc -l < "$WORKSPACE/.once/out/Host/Intermediates/preparation-runs.txt" | tr -d ' ')" = 4 ] || return 1
    [ -z "$(find "$WORKSPACE" -maxdepth 1 -name '*.swiftmodule' -print)" ] || return 1
    codesign --verify --deep --strict "$WORKSPACE/.once/out/Host/Host.app" 2> "$WORKSPACE/signature.err" || { cat "$WORKSPACE/signature.err"; return 1; }
  }

  It 'embeds a signed native extension and reuses explicit compiles across unrelated always-run directory outputs'
    Skip if 'requires Xcode' script_toolchain_unavailable
    When call exercise_native_extension
    The status should be success
    The output should equal ''
  End
End
