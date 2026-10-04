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
        (.cache == "miss" and (.outputs | has(".once/out/Script_App/Script App.app/_CodeSignature/CodeResources") or has(".once/out/Script_App/Script App.app/observed.txt"))))) and
      ($new | any(.cache == "bypass" and (.outputs | has(".once/out/Script_App/Script App.app"))))
    ' "$WORKSPACE/evidence-warm.json" >/dev/null || return
    [ "$(wc -l < "$WORKSPACE/.once/out/Script_App/Intermediates/runs.txt" | tr -d ' ')" = 2 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/undeclared.txt")" = 2 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/observed.txt")" = 2 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/Script_App/Script App.app/generated-resource.txt" ] || return 1
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$WORKSPACE/.once/out/Script_App/Script App.app/Info.plist")" = 42 ] || return 1
    [ "$("$WORKSPACE/.once/out/Script_App/Script App.app/Script App")" = 'script graph' ] || return 1
    codesign --verify "$WORKSPACE/.once/out/Script_App/Script App.app" 2> "$WORKSPACE/signature.err" || { cat "$WORKSPACE/signature.err"; return 1; }
    mv "$WORKSPACE/.once/out" "$WORKSPACE/saved-outputs"
    script_once build Script_App > "$WORKSPACE/restore.json" 2> "$WORKSPACE/build.err" || { cat "$WORKSPACE/build.err"; return 1; }
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$WORKSPACE/.once/out/Script_App/Script App.app/Info.plist")" = 42 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/from-script.txt")" = 'native graph inputs' ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/undeclared.txt")" = 1 ] || return 1
    [ "$(cat "$WORKSPACE/.once/out/Script_App/Script App.app/observed.txt")" = 1 ] || return 1
    [ ! -e "$WORKSPACE/.once/out/Script_App/Script App.app/generated-resource.txt" ] || return 1
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
End
