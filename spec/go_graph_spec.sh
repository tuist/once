#shellcheck shell=bash
# End-to-end specs for Go graph targets and locked vendored modules.

Describe 'Go graph'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  go_toolchain_unavailable() {
    ! command -v go >/dev/null 2>&1
  }

  copy_go_example() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/go-comprehensive/." "$WORKSPACE/"
  }

  exercise_go_starter() {
    once --format json query targets > "$WORKSPACE/targets.json" || return
    once --format json build --sandbox inputs Hello > "$WORKSPACE/build.json" || return

    hello_binary="$WORKSPACE/.once/out/Hello/Hello"
    if [ -f "$hello_binary.exe" ]; then
      hello_binary="$hello_binary.exe"
    fi
    "$hello_binary" > "$WORKSPACE/hello.txt" || return

    once --format json run Hello > "$WORKSPACE/run.json" || return
    once --format json test --sandbox inputs GreetingTests > "$WORKSPACE/full-test.json" || return
    cp "$WORKSPACE/.once/out/GreetingTests/test/test_results.json" \
      "$WORKSPACE/full-test-results.json" || return
    once --format json query test-manifest GreetingTests > "$WORKSPACE/test-manifest.json" || return
    once --format json test GreetingTests \
      --test-unit 'GreetingTests::TestMessage' > "$WORKSPACE/exact-test.json" || return
    manifest="$WORKSPACE/.once/test-manifests/GreetingTests/manifest.json"
    jq '.units += [{"id":"GreetingTests::DoesNotExist","name":"DoesNotExist","suite":"./internal/greeting"}]' \
      "$manifest" > "$WORKSPACE/manifest-with-missing-unit.json" || return
    cp "$WORKSPACE/manifest-with-missing-unit.json" "$manifest" || return
    missing_batch='0000000000000000000000000000000000000000000000000000000000000000'
    once --format json test GreetingTests \
      --batch-test-unit 'GreetingTests::DoesNotExist' \
      --test-batch-id "$missing_batch" > "$WORKSPACE/missing-test.json" || return
    once --format json build HelloLinux > "$WORKSPACE/cross-build.json"
  }

  It 'builds, runs, tests, and cross-compiles the starter with a vendored external module'
    Skip if 'Go toolchain unavailable on this host' go_toolchain_unavailable
    copy_go_example

    When call exercise_go_starter
    The status should be success
    The contents of file "$WORKSPACE/targets.json" should include '"kind":"go_module"'
    The contents of file "$WORKSPACE/targets.json" should include 'go-module-github.com_x47_pkg_x47_errors_x64_v0.9.1'
    The contents of file "$WORKSPACE/build.json" should include '"target":"Hello"'
    The contents of file "$WORKSPACE/hello.txt" should equal 'Hello, Once!'
    The contents of file "$WORKSPACE/run.json" should include '"target":"Hello"'
    The contents of file "$WORKSPACE/.once/out/Hello/run/stdout.log" should include 'Hello, Once!'
    The contents of file "$WORKSPACE/full-test-results.json" should include '"type":"go_test"'
    The contents of file "$WORKSPACE/full-test-results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/full-test-results.json" should include 'GreetingTests::TestEmptyName'
    The contents of file "$WORKSPACE/full-test-results.json" should include 'GreetingTests::TestMessage'
    The contents of file "$WORKSPACE/test-manifest.json" should include 'GreetingTests::BenchmarkMessage'
    The contents of file "$WORKSPACE/.once/out/GreetingTests/test/test_results.json" should include '"total":1'
    The contents of file "$WORKSPACE/.once/out/GreetingTests/test/test_results.json" should include 'GreetingTests::TestMessage'
    The contents of file "$WORKSPACE/.once/out/GreetingTests/test/test_results.json" should not include 'GreetingTests::TestEmptyName'
    The contents of file "$WORKSPACE/.once/out/GreetingTests/test/batches/0000000000000000000000000000000000000000000000000000000000000000/test_results.json" should not include 'GreetingTests::DoesNotExist'
    The path "$WORKSPACE/.once/out/GreetingTests/test/coverage.out" should be file
    The path "$WORKSPACE/.once/out/GreetingTests/test/native_results.jsonl" should be file
    The path "$WORKSPACE/.once/out/HelloLinux/HelloLinux" should be file
  End
End
