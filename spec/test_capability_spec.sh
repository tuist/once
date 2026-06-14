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
    cp -R "$REPO_ROOT/fixtures/shellspec_test/." "$WORKSPACE/"
    sed -e "s|__SHELLSPEC_BIN__|$shellspec_bin|g" \
      -e "s|__CLEAN_PATH__|$clean_path|g" \
      -e "s|__ONCE_BIN__|$ONCE_BIN|g" \
      "$REPO_ROOT/fixtures/shellspec_test/once.toml" > "$WORKSPACE/once.toml"
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
