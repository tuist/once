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
End
