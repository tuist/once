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

  setup_release_graph_workspace() {
    mkdir -p \
      "$WORKSPACE/crates/once-cas" \
      "$WORKSPACE/crates/once-core" \
      "$WORKSPACE/crates/once-frontend" \
      "$WORKSPACE/crates/once" \
      "$WORKSPACE/crates/once-cli"
    touch \
      "$WORKSPACE/Cargo.toml" \
      "$WORKSPACE/Cargo.lock" \
      "$WORKSPACE/once.toml" \
      "$WORKSPACE/crates/once-cas/once.toml" \
      "$WORKSPACE/crates/once-core/once.toml" \
      "$WORKSPACE/crates/once-frontend/once.toml" \
      "$WORKSPACE/crates/once/once.toml" \
      "$WORKSPACE/crates/once-cli/once.toml"
  }

  It 'copies a built shared library into each SDK prebuild directory'
    setup_release_path
    setup_release_graph_workspace
    write_stub cargo \
      '#!/usr/bin/env bash' \
      'case "$1" in' \
      '  vendor)' \
      '    mkdir -p third_party/rust/vendor' \
      '    ;;' \
      '  metadata)' \
      '    printf "%s\n" "{\"packages\":[]}"' \
      '    ;;' \
      'esac'
    write_stub mise \
      '#!/usr/bin/env bash' \
      'if [ "$1" = exec ] && [ "$2" = -- ]; then' \
      '  shift 2' \
      '  exec "$@"' \
      'fi' \
      'exit 2'
    write_stub jq \
      '#!/usr/bin/env bash'
    write_stub once \
      '#!/usr/bin/env bash' \
      'target_id="$2"' \
      'mkdir -p ".once/out/${target_id}"' \
      'printf library > ".once/out/${target_id}/libonce.so"'
    mkdir -p "$WORKSPACE/packages/js" "$WORKSPACE/packages/ruby"
    cd "$WORKSPACE"

    When call env ONCE_BIN="$WORKSPACE/bin/once" "$REPO_ROOT/mise/tasks/release/package-sdk-libs.sh" --target x86_64-unknown-linux-gnu
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

  It 'rejects publish versions with empty prerelease metadata'
    setup_release_path
    write_stub gem '#!/usr/bin/env bash'
    write_stub npm '#!/usr/bin/env bash'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/publish-sdks.sh" --version 1.2.3-
    The status should be failure
    The stderr should include '--version must be a semantic version'
  End

  It 'rejects publish versions with empty build metadata'
    setup_release_path
    write_stub gem '#!/usr/bin/env bash'
    write_stub npm '#!/usr/bin/env bash'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/publish-sdks.sh" --version 1.2.3+
    The status should be failure
    The stderr should include '--version must be a semantic version'
  End

  It 'rejects publish versions with malformed prerelease metadata'
    setup_release_path
    write_stub gem '#!/usr/bin/env bash'
    write_stub npm '#!/usr/bin/env bash'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/publish-sdks.sh" --version 1.2.3-+foo
    The status should be failure
    The stderr should include '--version must be a semantic version'
  End

  It 'accepts publish versions with prerelease and build metadata'
    setup_release_path
    write_stub gem '#!/usr/bin/env bash'
    write_stub npm '#!/usr/bin/env bash'
    cd "$WORKSPACE"

    When call "$REPO_ROOT/mise/tasks/release/publish-sdks.sh" --version 1.2.3-beta.1+build.5
    The status should be failure
    The stderr should include 'SDK prebuilds are missing'
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
