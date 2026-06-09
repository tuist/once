#shellcheck shell=bash
# Release script checks for SDK packaging and publishing validation.

Describe 'release SDK packaging scripts'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  setup_release_path() {
    mkdir -p "$WORKSPACE/bin"
    PATH="$WORKSPACE/bin:$PATH"
    export PATH
  }

  write_stub() {
    path="$WORKSPACE/bin/$1"
    shift
    printf '%s\n' "$@" > "$path"
    chmod +x "$path"
  }

  It 'copies a built shared library into each SDK prebuild directory'
    setup_release_path
    write_stub cargo \
      '#!/usr/bin/env bash' \
      'mkdir -p target/x86_64-unknown-linux-gnu/release' \
      'printf library > target/x86_64-unknown-linux-gnu/release/libonce.so'
    mkdir -p "$WORKSPACE/packages/js" "$WORKSPACE/packages/ruby"
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/package-sdk-libs.sh" --target x86_64-unknown-linux-gnu
    The status should be success
    The contents of file "$WORKSPACE/packages/js/prebuilds/linux-x64/libonce.so" should equal 'library'
    The contents of file "$WORKSPACE/packages/ruby/prebuilds/linux-x64/libonce.so" should equal 'library'
  End

  It 'rejects unsupported SDK package targets'
    setup_release_path
    write_stub cargo \
      '#!/usr/bin/env bash' \
      'mkdir -p target/unknown-target/release' \
      'printf library > target/unknown-target/release/libonce.so'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/package-sdk-libs.sh" --target unknown-target
    The status should be failure
    The stderr should include 'unsupported SDK package target: unknown-target'
  End

  It 'rejects publish versions that are not semantic versions'
    setup_release_path
    write_stub gem '#!/usr/bin/env bash'
    write_stub npm '#!/usr/bin/env bash'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/publish-sdks.sh" --version latest
    The status should be failure
    The stderr should include '--version must be a semantic version'
  End

  It 'checks SDK prebuilds before publishing'
    setup_release_path
    write_stub gem '#!/usr/bin/env bash'
    write_stub npm '#!/usr/bin/env bash'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/publish-sdks.sh" --version 0.1.0
    The status should be failure
    The stderr should include 'SDK prebuilds are missing'
  End
End
