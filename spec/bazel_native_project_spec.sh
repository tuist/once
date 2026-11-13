#shellcheck shell=bash
# End-to-end specs for the graph Once derives from a Bazel workspace.

Describe 'bazel native project'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  setup_bazel_workspace() {
    cp -R "$REPO_ROOT/fixtures/bazel_rules_rust/." "$WORKSPACE/"
    chmod +x "$WORKSPACE/tools/bazel" "$WORKSPACE/tools/bazelisk"
  }

  target_ids() {
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" --format json query targets |
      jq -r '.[] | "\(.kind) \(.id)"' |
      sort
  }

  It 'lets an owning Bazel root suppress nested ecosystem examples'
    setup_bazel_workspace

    When call target_ids
    The status should be success
    The stdout should include 'bazel_workspace bazel'
    The stdout should include 'bazel_target bz_support'
    The stdout should include 'bazel_test bz_support_test'
    The stdout should not include 'cargo_workspace'
    The stdout should not include 'swift_package_workspace'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'builds and tests the complete workspace without a Once manifest'
    setup_bazel_workspace

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" test --quiet
    The status should be success
    The stdout should include 'test batches'
    The contents of file "$WORKSPACE/.once/out/bz_support_test/test/test_results.json" should include '"status":"passed"'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'builds the complete workspace through the detected root'
    setup_bazel_workspace

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" build --quiet
    The status should be success
    The stdout should include 'once: build bazel (bazel_workspace)'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'recognizes a WORKSPACE.bazel-only repository'
    setup_bazel_workspace
    mv "$WORKSPACE/MODULE.bazel" "$WORKSPACE/WORKSPACE.bazel"

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" --format json query targets
    The status should be success
    The stdout should include '"id":"bazel"'
    The stdout should include '"id":"bz_support"'
    The stdout should include '"id":"bz_support_test"'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'uses the native test strategy when TestRunner argv cannot run on its own'
    setup_bazel_workspace
    cat > "$WORKSPACE/tools/bazelisk" <<'SH'
#!/bin/sh
case "$1" in
  --version) printf 'bazel 8.4.2\n' ;;
  query) printf 'sh_test rule //:support_test\n' ;;
  aquery) printf '%s\n' '{"actions":[{"mnemonic":"TestRunner","arguments":["/bin/sh","-c","exit 42"]}]}' ;;
  info) printf '%s\n' "${TMPDIR:-/tmp}/once-bazel-output-base" ;;
  fetch|test) exit 0 ;;
  *) exit 2 ;;
esac
SH

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" --format json test bz_support_test
    The status should be success
    The stderr should be blank
    The stdout should include '"status":"completed"'
    The stdout should include '"cache":"bypass"'
  End

  owned_cache_roundtrip() {
    printf alpha > "$WORKSPACE/value.txt"
    cat > "$WORKSPACE/tools/copy" <<SH
#!/bin/sh
printf x >> "$WORKSPACE/executions"
/bin/cat "\$1" >> "\$2"
SH
    chmod +x "$WORKSPACE/tools/copy"
    cat > "$WORKSPACE/once.toml" <<TOML
[[target]]
name = "bazel"
kind = "bazel_workspace"
srcs = ["MODULE.bazel"]
[target.attrs]
toolchain_inputs = ["$WORKSPACE/tools/copy", "/bin/sh", "/bin/cat"]
TOML
    cat > "$WORKSPACE/tools/bazelisk" <<SH
#!/bin/sh
case "\$1" in
  --version) printf 'bazel 9\n' ;;
  query) printf 'cc_library rule //:support\n' ;;
  info) printf '%s\n' "$WORKSPACE/native" ;;
  cquery) printf 'bazel-out/alias.txt\n' ;;
  aquery) cat <<'JSON'
{"pathFragments":[{"id":1,"label":"value.txt"},{"id":2,"label":"bazel-out/value.txt"},{"id":3,"label":"bazel-out/alias.txt"},{"id":4,"label":"bazel-out/unused"}],"artifacts":[{"id":1,"pathFragmentId":1},{"id":2,"pathFragmentId":2},{"id":3,"pathFragmentId":3},{"id":4,"pathFragmentId":4}],"depSetOfFiles":[{"id":1,"directArtifactIds":[1]},{"id":2,"directArtifactIds":[2]}],"actions":[{"mnemonic":"Copy","arguments":["$WORKSPACE/tools/copy","value.txt","bazel-out/value.txt"],"inputDepSetIds":[1],"outputIds":[2]},{"mnemonic":"SolibSymlink","inputDepSetIds":[2],"outputIds":[3]},{"mnemonic":"FileWrite","outputIds":[4]}]}
JSON
    ;;
  *) exit 42 ;;
esac
SH
    local output="$WORKSPACE/.once/bazel-actions/bazel-out/alias.txt"
    mkdir -p "$WORKSPACE/.once/bazel-actions/bazel-out"
    printf unrelated > "$WORKSPACE/.once/bazel-actions/bazel-out/private"
    chmod 000 "$WORKSPACE/.once/bazel-actions/bazel-out/private"
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" build bz_support --quiet > "$WORKSPACE/run.log" || return
    [ "$(cat "$output")" = alpha ] || return 1
    [ "$(cat "$WORKSPACE/executions")" = x ] || return 1
    [ ! -e "$WORKSPACE/.once/bazel-actions/bazel-out/unused" ] || return 1
    rm -rf "$WORKSPACE/.once/bazel-actions"
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" build bz_support --quiet > "$WORKSPACE/run.log" || return
    [ "$(cat "$output")" = alpha ] || return 1
    [ "$(cat "$WORKSPACE/executions")" = x ] || return 1
    printf beta > "$WORKSPACE/value.txt"
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" build bz_support --quiet > "$WORKSPACE/run.log" || return
    [ "$(cat "$output")" = beta ] || return 1
    [ "$(cat "$WORKSPACE/executions")" = xx ] || return 1
    printf 'restored without execution; invalidated changed input\n'
  }

  It 'restores owned outputs from cache and invalidates changed sources'
    setup_bazel_workspace

    When call owned_cache_roundtrip
    The status should be success
    The stdout should equal 'restored without execution; invalidated changed input'
    The stderr should be blank
  End

  owned_test_runfiles() {
    mkdir -p "$WORKSPACE/hidden"
    printf payload > "$WORKSPACE/hidden/source.txt"
    cat > "$WORKSPACE/launcher.sh" <<'SH'
#!/bin/sh
test_binary="$1"
shift
exec "$TEST_SRCDIR/_main/$test_binary" "$@"
SH
    cat > "$WORKSPACE/test.sh" <<SH
#!/bin/sh
[ "\$#" = 1 ] && [ "\$1" = expected ] && [ "\$DECLARED" = runtime ] &&
[ "\$TEST_TOTAL_SHARDS" = 2 ] &&
[ "\$(cat "\$TEST_SRCDIR/_main/data/renamed.txt")" = payload ] &&
[ -f "\$TEST_SRCDIR/_main/empty" ] || exit 1
printf '%s\n' "\$TEST_SHARD_INDEX" >> "$WORKSPACE/shards"
SH
    chmod +x "$WORKSPACE/launcher.sh" "$WORKSPACE/test.sh"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "bazel"
kind = "bazel_workspace"
srcs = ["MODULE.bazel"]
[target.attrs.test_env]
DECLARED = "runtime"
TOML
    cat > "$WORKSPACE/tools/bazelisk" <<SH
#!/bin/sh
case "\$1" in
  --version) printf 'bazel 9\n' ;;
  query)
    case "\$*" in
      *streamed_jsonproto*) printf '%s\n' '{"rule":{"attribute":[{"name":"shard_count","intValue":2},{"name":"args","stringListValue":["expected"]}]}}' ;;
      *) printf 'sh_test rule //:support_test\n' ;;
    esac ;;
  info) printf '%s\n' "$WORKSPACE/native" ;;
  cquery) printf 'bazel-out/test.runfiles\n' ;;
  aquery) cat <<JSON
{"pathFragments":[{"id":1,"label":"launcher.sh"},{"id":2,"label":"test.sh"},{"id":3,"label":"hidden/source.txt"},{"id":4,"label":"bazel-out/test.runfiles_manifest"},{"id":5,"label":"bazel-out/test.runfiles/MANIFEST"},{"id":6,"label":"bazel-out/test.runfiles"},{"id":7,"label":"bazel-out/testlogs/shard_1/test.log"},{"id":8,"label":"bazel-out/testlogs/shard_2/test.log"}],
"artifacts":[{"id":1,"pathFragmentId":1},{"id":2,"pathFragmentId":2},{"id":3,"pathFragmentId":3},{"id":4,"pathFragmentId":4},{"id":5,"pathFragmentId":5},{"id":6,"pathFragmentId":6},{"id":7,"pathFragmentId":7},{"id":8,"pathFragmentId":8}],
"depSetOfFiles":[{"id":1,"directArtifactIds":[4]},{"id":2,"directArtifactIds":[5,2,3]},{"id":3,"directArtifactIds":[1,6]}],
"actions":[
{"mnemonic":"SourceSymlinkManifest","outputIds":[4],"fileContents":"_main/bin/test.sh $WORKSPACE/test.sh\\n_main/data/renamed.txt $WORKSPACE/hidden/source.txt\\n_main/empty \\n"},
{"mnemonic":"SymlinkTree","inputDepSetIds":[1],"outputIds":[5]},
{"mnemonic":"RunfilesTree","inputDepSetIds":[2],"outputIds":[6]},
{"mnemonic":"TestRunner","arguments":["./launcher.sh","bin/test.sh","expected"],"inputDepSetIds":[3],"outputIds":[7]},
{"mnemonic":"TestRunner","arguments":["./launcher.sh","bin/test.sh","expected"],"inputDepSetIds":[3],"outputIds":[8]}]}
JSON
    ;;
  *) exit 42 ;;
esac
SH
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" test bz_support_test --quiet > "$WORKSPACE/run.log" || return
    [ "$(sort "$WORKSPACE/shards" | tr '\n' ',')" = '0,1,' ] || return 1
    local tree="$WORKSPACE/.once/bazel-actions/bazel-out/test.runfiles"
    printf stale > "$tree/stale"
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" build bz_support_test --quiet > "$WORKSPACE/run.log" || return
    [ ! -e "$tree/stale" ] || return 1
    rm "$WORKSPACE/shards"
    rm -rf "$WORKSPACE/.once/bazel-actions"
    env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" test bz_support_test --quiet > "$WORKSPACE/run.log" || return
    [ "$(sort "$WORKSPACE/shards" | tr '\n' ',')" = '0,1,' ] || return 1
    printf 'ran both shards with mapped runfiles after restore\n'
  }

  It 'runs owned test shards with manifest-mapped runfiles and explicit environment'
    setup_bazel_workspace

    When call owned_test_runfiles
    The status should be success
    The stdout should equal 'ran both shards with mapped runfiles after restore'
    The stderr should be blank
  End
End
