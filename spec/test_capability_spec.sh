#shellcheck shell=bash
# End-to-end specs for generic test capability targets.

Describe 'test capability'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_shellspec_test_workspace() {
    shellspec_bin="shellspec"
    clean_path=""
    old_ifs="$IFS"
    IFS=:
    for path_entry in $PATH; do
      case "$path_entry" in
        /var/folders/*/T/shellspec.*|*/spec/support/bin)
          continue
          ;;
        */mise/installs/shellspec/*)
          shellspec_bin="$path_entry/shellspec"
          ;;
      esac
      if [ -z "$clean_path" ]; then
        clean_path="$path_entry"
      else
        clean_path="$clean_path:$path_entry"
      fi
    done
    IFS="$old_ifs"
    mkdir -p "$WORKSPACE/spec"
    cat > "$WORKSPACE/once.toml" <<EOF
[[target]]
name = "cli_e2e"
kind = "shellspec_test"
srcs = ["spec/**/*_spec.sh"]

[target.attrs]
shellspec = "$shellspec_bin"
data = ["spec/spec_helper.sh"]
labels = ["e2e", "shell"]
env = { PATH = "$clean_path", ONCE_BIN = "$ONCE_BIN" }
EOF
    cat > "$WORKSPACE/spec/spec_helper.sh" <<'EOF'
#shellcheck shell=bash

setup_nested_workspace() {
  NESTED_WORKSPACE="$(mktemp -d -t once-nested-spec.XXXXXX)"
  export NESTED_WORKSPACE
}

cleanup_nested_workspace() {
  if [ -n "${NESTED_WORKSPACE:-}" ] && [ -d "$NESTED_WORKSPACE" ]; then
    rm -rf "$NESTED_WORKSPACE"
    unset NESTED_WORKSPACE
  fi
}

once_under_test() {
  "$ONCE_BIN" -C "$NESTED_WORKSPACE" "$@"
}
EOF
    printf '%s\n' \
      '#shellcheck shell=bash' \
      '' \
      "Describe 'once cli surface'" \
      "  Include './spec/spec_helper.sh'" \
      "  BeforeEach 'setup_nested_workspace'" \
      "  AfterEach 'cleanup_nested_workspace'" \
      '' \
      "  It 'lists commands'" \
      '    When call once_under_test --list' \
      '    The status should be success' \
      "    The stdout should include 'test'" \
      "    The stdout should include 'query'" \
      '  End' \
      'End' \
      > "$WORKSPACE/spec/cli_surface_spec.sh"
    printf '%s\n' \
      '#shellcheck shell=bash' \
      '' \
      "Describe 'once graph query'" \
      "  Include './spec/spec_helper.sh'" \
      "  BeforeEach 'setup_nested_workspace'" \
      "  AfterEach 'cleanup_nested_workspace'" \
      '' \
      "  It 'lists declared targets'" \
      '    {' \
      "      printf '%s\\n' '[['target']]'" \
      "      printf '%s\\n' 'name = \"smoke\"'" \
      "      printf '%s\\n' 'kind = \"shellspec_test\"'" \
      "      printf '%s\\n' 'srcs = [\"spec/**/*_spec.sh\"]'" \
      '    } > "$NESTED_WORKSPACE/once.toml"' \
      '' \
      '    When call once_under_test query targets' \
      '    The status should be success' \
      "    The stdout should include 'smoke (shellspec_test) [test]'" \
      '  End' \
      'End' \
      > "$WORKSPACE/spec/graph_query_spec.sh"
  }

  It 'discovers shellspec test targets through the generic test query'
    create_shellspec_test_workspace

    When call once --format json query tests
    The status should be success
    The stdout should include '"id":"cli_e2e"'
    The stdout should include '"kind":"shellspec_test"'
    The stdout should include '"runner":"shellspec"'
    The stdout should include '"results_path":".once/out/cli_e2e/test/test_results.json"'
  End

  It 'selects affected shellspec tests from changed inputs'
    create_shellspec_test_workspace

    When call once --format json query affected-tests --changed-path spec/cli_surface_spec.sh
    The status should be success
    The stdout should include '"id":"cli_e2e"'
    The stdout should include 'changed test input `spec/cli_surface_spec.sh`'
  End

  It 'runs affected shellspec tests through the MCP tool'
    create_shellspec_test_workspace
    mcp_request='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"once_run_tests","arguments":{"changed_paths":["spec/cli_surface_spec.sh"]}}}'

    When call /bin/sh -c 'printf "%s\n" "$1" | "$2" -C "$3" mcp' sh "$mcp_request" "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include 'cli_e2e'
    The stdout should include 'once.test_results.v1'
    The stdout should include '\"success\": true'
  End

  It 'runs shellspec tests and exposes normalized test results'
    create_shellspec_test_workspace

    When call once --format json test cli_e2e
    The status should be success
    The stdout should include '"target":"cli_e2e"'
    The stdout should include '"capability":"test"'
    The path "$WORKSPACE/.once/out/cli_e2e/test/test_results.json" should be file
  End

  It 'queries normalized shellspec test results'
    create_shellspec_test_workspace
    once --format json test cli_e2e >/dev/null

    When call once --format json query test-results cli_e2e
    The status should be success
    The stdout should include '"schema":"once.test_results.v1"'
    The stdout should include '"target":"cli_e2e"'
    The stdout should include '"status":"passed"'
    The stdout should include 'spec/cli_surface_spec.sh::lists commands'
    The stdout should include 'spec/graph_query_spec.sh::lists declared targets'
  End
End
