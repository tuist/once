#shellcheck shell=bash

verify_starter_example() {
  kind="$1"
  slug="$2"
  capability="$3"
  root="$WORKSPACE/starters/$kind-$slug"
  mkdir -p "$root"

  "$ONCE_BIN" -C "$root" edit materialize-example "$kind" "$slug" >/dev/null || return
  target_ids="$(
    "$ONCE_BIN" -C "$root" --format json query targets |
      jq -r --arg kind "$kind" '.[] | select(.kind == $kind) | .id'
  )" || return
  if [ -z "$target_ids" ]; then
    echo "starter $kind $slug created no matching target" >&2
    return 1
  fi

  while IFS= read -r target_id; do
    [ -n "$target_id" ] || continue
    "$ONCE_BIN" -C "$root" "$capability" "$target_id" --quiet || return
  done <<EOF
$target_ids
EOF

  printf 'verified %s %s with %s\n' "$kind" "$slug" "$capability"
}

verify_portable_starter_examples() {
  while IFS='|' read -r kind slug capability; do
    [ -n "$kind" ] || continue
    verify_starter_example "$kind" "$slug" "$capability" || return
  done <<'EOF'
c_library|c-library-minimal|build
elixir_library|elixir-library-minimal|build
elixir_test|elixir-test-minimal|test
go_binary|go-binary-minimal|build
go_binary|go-comprehensive|build
go_test|go-comprehensive|test
jest_test|jest-test-minimal|test
kotlin_jvm_binary|kotlin-jvm-binary-minimal|build
kotlin_jvm_library|kotlin-jvm-library-minimal|build
kotlin_jvm_test|kotlin-jvm-test-minimal|test
minitest_test|minitest-test-minimal|test
pytest_test|pytest-test-minimal|test
rspec_test|rspec-test-minimal|test
rust_binary|rust-binary-minimal|build
rust_binary|rust-binary-with-crate|build
rust_library|rust-library-minimal|build
rust_proc_macro|rust-proc-macro-minimal|build
rust_test|rust-test-minimal|build
shellspec_test|shellspec-test-minimal|test
vitest_test|vitest-test-minimal|test
zig_binary|zig-binary-with-library|build
zig_c_library|zig-c-library-with-c-provider|build
zig_configure|zig-configure-library-minimal|build
zig_configure_binary|zig-configure-binary-minimal|build
zig_configure_test|zig-configure-test-minimal|build
zig_dependencies|zig-binary-with-package|build
zig_library|zig-library-minimal|build
zig_shared_library|zig-shared-library-minimal|build
zig_static_library|zig-static-library-minimal|build
zig_test|zig-test-minimal|build
EOF
}

Describe 'portable starter examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'materializes and executes every portable starter'
    When call verify_portable_starter_examples
    The status should be success
    The stdout should include 'verified c_library c-library-minimal with build'
    The stdout should include 'verified pytest_test pytest-test-minimal with test'
    The stdout should include 'verified rust_binary rust-binary-minimal with build'
    The stdout should include 'verified zig_test zig-test-minimal with build'
  End
End
