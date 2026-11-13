#shellcheck shell=bash

Describe 'CTest graph target kind'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  cmake_tools_unavailable() {
    ! command -v cmake >/dev/null 2>&1 || ! command -v ctest >/dev/null 2>&1 || ! command -v ninja >/dev/null 2>&1 || ! command -v cc >/dev/null 2>&1
  }

  write_ctest_workspace() {
    cp "$REPO_ROOT/crates/once-frontend/prelude/examples/cmake-test-minimal/"* "$WORKSPACE/"
  }

  restore_ctest_tree() {
    write_ctest_workspace
    once --format json build Tests || return
    rm -rf "$WORKSPACE/.once/out/Tests/cmake-build"
    once --format json build Tests || return
    once --format json test Tests || return
    cat "$WORKSPACE/.once/out/Tests/test/test_results.json"
  }

  It 'rebuilds a deleted native test tree and reports one passing suite'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call restore_ctest_tree
    The status should be success
    The stdout should include '"cache":"bypass"'
    The stdout should include '"total":1,"passed":1,"failed":0'
    The contents of file "$WORKSPACE/.once/out/Tests/test/ctest.xml" should include 'check_sum'
  End

  fail_ctest_suite() {
    write_ctest_workspace
    printf '%s\n' 'int main(void) { return 7; }' > "$WORKSPACE/test.c"
    once --format json test Tests > "$WORKSPACE/failure.json" 2>&1
    code=$?
    cat "$WORKSPACE/.once/out/Tests/test/test_results.json"
    return "$code"
  }

  It 'propagates native failure while retaining normalized failed results'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call fail_ctest_suite
    The status should be failure
    The stdout should include '"total":1,"passed":0,"failed":1'
    The contents of file "$WORKSPACE/.once/out/Tests/test/ctest.xml" should include '<failure'
  End

  fail_empty_ctest_suite() {
    write_ctest_workspace
    printf '%s\n' 'cmake_minimum_required(VERSION 3.21)' 'project(Empty C)' 'enable_testing()' > "$WORKSPACE/CMakeLists.txt"
    once --format json test Tests > "$WORKSPACE/failure.json" 2>&1
    code=$?
    cat "$WORKSPACE/.once/out/Tests/test/test_results.json"
    return "$code"
  }

  It 'fails an empty native suite rather than claiming a passing test'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call fail_empty_ctest_suite
    The status should be failure
    The stdout should include '"status":"failed"'
  End

  run_custom_cmake_suite() {
    mode="$1"
    write_ctest_workspace
    cat > "$WORKSPACE/CMakeLists.txt" <<'CMAKE'
cmake_minimum_required(VERSION 3.21)
project(CustomTests C)
add_executable(check_sum test.c)
add_custom_target(native-check COMMAND check_sum DEPENDS check_sum)
add_custom_target(second-check COMMAND "${CMAKE_COMMAND}" -E echo "second suite executed")
add_custom_target(slow-check COMMAND "${CMAKE_COMMAND}" -E sleep 3)
CMAKE
    targets='"native-check", "second-check"'
    case "$mode" in
      failure) printf '%s\n' 'int main(void) { return 7; }' > "$WORKSPACE/test.c" ;;
      missing) targets='"missing-check"' ;;
      timeout) targets='"slow-check"'; printf '%s\n' 'timeout_ms = 100' >> "$WORKSPACE/once.toml" ;;
    esac
    printf 'test_targets = [%s]\nctest = "nonexistent-ctest"\n' "$targets" >> "$WORKSPACE/once.toml"
    once --format json test Tests > "$WORKSPACE/run.json" 2>&1
    code=$?
    cat "$WORKSPACE/.once/out/Tests/test/test_results.json"
    return "$code"
  }

  It 'runs explicit native test targets without CTest and retains logs without invented JUnit'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call run_custom_cmake_suite success
    The status should be success
    The stdout should include '"total":1,"passed":1,"failed":0'
    The stdout should include '"native_results":[]'
    The contents of file "$WORKSPACE/.once/out/Tests/test/cmake-test.log" should include 'second suite executed'
    The path "$WORKSPACE/.once/out/Tests/test/ctest.xml" should not be exist
  End

  Describe 'custom native test failures'
    Parameters
      failure
      missing
      timeout
    End

    It "retains normalized failure for $1"
      Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
      When call run_custom_cmake_suite "$1"
      The status should be failure
      The stdout should include '"total":1,"passed":0,"failed":1'
      The contents of file "$WORKSPACE/.once/out/Tests/test/cmake-test.log" should include 'CMake test runner failed:'
    End
  End

  reject_relocated_ctest_tree() {
    write_ctest_workspace
    once --format json build Tests > "$WORKSPACE/original.json" || return
    mkdir "$WORKSPACE/other"
    cp "$WORKSPACE/once.toml" "$WORKSPACE/CMakeLists.txt" "$WORKSPACE/test.c" "$WORKSPACE/other/"
    cp -R "$WORKSPACE/.once" "$WORKSPACE/other/.once"
    "$ONCE_BIN" -C "$WORKSPACE/other" --format json build Tests > "$WORKSPACE/relocated.json" || return
    "$SPEC_PYTHON3" - "$WORKSPACE" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
original = json.loads((root / 'original.json').read_text())
relocated = json.loads((root / 'relocated.json').read_text())
assert original['action_digest'] != relocated['action_digest']
assert relocated['cache'] == 'bypass', relocated
cache = (root / 'other/.once/out/Tests/cmake-build/CMakeCache.txt').read_text()
assert str(root / 'other') in cache
print('different workspace: distinct actual key and cache bypass')
PY
  }

  It 'does not reuse a captured native build tree at a different workspace path'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call reject_relocated_ctest_tree
    The status should be success
    The stdout should include 'distinct actual key and cache bypass'
  End

  reject_sandboxed_ctest() {
    write_ctest_workspace
    once --format json build --sandbox copied-inputs Tests
  }

  It 'rejects sandbox relocation instead of caching an invalid native tree'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call reject_sandboxed_ctest
    The status should be failure
    The stderr should include 'requires execution at its original workspace path'
    The stderr should include 'session failed'
  End

  timeout_ctest_suite() {
    write_ctest_workspace
    printf '%s\n' 'timeout_ms = 100' >> "$WORKSPACE/once.toml"
    printf '%s\n' 'add_test(NAME slow COMMAND "${CMAKE_COMMAND}" -E sleep 3)' >> "$WORKSPACE/CMakeLists.txt"
    once --format json test Tests > "$WORKSPACE/failure.json" 2>&1
    code=$?
    cat "$WORKSPACE/.once/out/Tests/test/test_results.json"
    return "$code"
  }

  It 'enforces a suite timeout and retains normalized failure results'
    Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
    When call timeout_ctest_suite
    The status should be failure
    The stdout should include '"status":"failed"'
    The contents of file "$WORKSPACE/.once/out/Tests/test/ctest.log" should include 'Process terminated due to timeout'
  End

  rebuild_changed_cmake_toolchain() {
    kind="$1"
    selection="$2"
    write_ctest_workspace
    real_cc="$(command -v cc)"
    mkdir "$WORKSPACE/sdk"
    cat > "$WORKSPACE/test.c" <<'C'
#include <stdio.h>
int main(void) { printf("%d\n", BUILD_VALUE); return 0; }
C
    if [ "$selection" = compiler ]; then
      configure="-DCMAKE_C_COMPILER:FILEPATH=$WORKSPACE/sdk/compiler"
    else
      configure="-DCMAKE_TOOLCHAIN_FILE:FILEPATH=$WORKSPACE/sdk/toolchain.cmake"
      printf '#!/bin/sh\nexec "%s" "$@"\n' "$real_cc" > "$WORKSPACE/sdk/compiler"
      chmod +x "$WORKSPACE/sdk/compiler"
    fi
    cat > "$WORKSPACE/once.toml" <<TOML
[[target]]
name = "Tests"
kind = "$kind"
srcs = ["CMakeLists.txt", "test.c"]
[target.attrs]
configure_args = ["$configure"]
TOML
    if [ "$kind" = cmake_project ]; then
      printf '%s\n' 'products = ["check_sum"]' >> "$WORKSPACE/once.toml"
      binary="$WORKSPACE/.once/out/Tests/products/check_sum"
    else
      binary="$WORKSPACE/.once/out/Tests/cmake-build/check_sum"
    fi
    for value in 11 29; do
      if [ "$selection" = compiler ]; then
        printf '#!/bin/sh\nexec "%s" -DBUILD_VALUE=%s "$@"\n' "$real_cc" "$value" > "$WORKSPACE/sdk/compiler"
        chmod +x "$WORKSPACE/sdk/compiler"
      else
        printf 'set(CMAKE_C_COMPILER "%s/sdk/compiler")\nset(CMAKE_C_FLAGS_INIT "-DBUILD_VALUE=%s")\n' "$WORKSPACE" "$value" > "$WORKSPACE/sdk/toolchain.cmake"
      fi
      once --format json build Tests > "$WORKSPACE/build-$value.json" || return
      "$binary" > "$WORKSPACE/value-$value.txt" || return
    done
    first="$(cat "$WORKSPACE/value-11.txt")"
    second="$(cat "$WORKSPACE/value-29.txt")"
    printf '%s %s: %s -> %s\n' "$kind" "$selection" "$first" "$second"
    [ "$first" = 11 ] && [ "$second" = 29 ]
  }

  Describe 'selected native toolchain changes'
    Parameters
      cmake_test compiler
      cmake_test toolchain
      cmake_project compiler
      cmake_project toolchain
    End

    It "rebuilds $1 when an external $2 changes at the same path"
      Skip if 'CMake native toolchain unavailable' cmake_tools_unavailable
      When call rebuild_changed_cmake_toolchain "$1" "$2"
      The status should be success
      The stdout should include '11 -> 29'
    End
  End

End
