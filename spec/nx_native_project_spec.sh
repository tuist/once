#shellcheck shell=bash
# End-to-end specs for the graph Once derives from an Nx workspace without a
# manifest of its own. `nx graph --print` returns the project graph that Once
# translates into `nx_task` targets so it can schedule and cache them.

Describe 'nx native project'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  nx_toolchain_unavailable() {
    command -v node >/dev/null 2>&1 || return 0
    command -v npm >/dev/null 2>&1 || return 0
    return 1
  }

  copy_fixture() {
    cp -R "$REPO_ROOT/fixtures/$1/." "$WORKSPACE/"
  }

  install_fixture() {
    (cd "$WORKSPACE" && npm install --no-audit --no-fund --loglevel=error >/dev/null 2>&1)
  }

  target_ids() {
    once --format json query targets | jq -r '.[] | "\(.kind) \(.id)"' | sort
  }

  rebuild_without_output() {
    once --format json build probe || return
    rm "$WORKSPACE/packages/app/result.txt"
    once --format json build probe
  }

  optional_outputs() {
    result=$(once --format json build probe) || return
    printf '%s\n' "$result" | jq -er '.cache == "miss"' >/dev/null || return
    test ! -e "$WORKSPACE/packages/app/coverage" || return
    result=$(once --format json build probe) || return
    printf '%s\n' "$result" | jq -er '.cache == "hit"' >/dev/null || return
    printf present > "$WORKSPACE/packages/app/mode"
    result=$(once --format json build probe) || return
    printf '%s\n' "$result" | jq -er '.cache == "miss"' >/dev/null || return
    test -f "$WORKSPACE/packages/app/coverage/result.txt" || return
    rm -rf "$WORKSPACE/packages/app/coverage"
    result=$(once --format json build probe) || return
    printf '%s\n' "$result" | jq -er '.cache == "hit"' >/dev/null || return
    printf fail > "$WORKSPACE/packages/app/mode"
    if once --format json build probe > "$WORKSPACE/failure.stdout" 2> "$WORKSPACE/failure.stderr"; then
      return 1
    fi
    grep -q 'exit code 13' "$WORKSPACE/failure.stderr" || return
    printf 'miss hit miss hit; failure propagated\n'
  }

  It 'derives Nx tasks with no once.toml present'
    nx_toolchain_unavailable && Skip 'node and npm are required'
    copy_fixture nx_hello
    install_fixture || Skip 'npm install failed; skipping (offline?)'

    When call target_ids
    The status should be success
    The stdout should include 'nx_workspace nx'
    The stdout should include 'nx_task hello_world__build'
    The stdout should include 'nx_task hello_world__test'
    The stdout should include 'nx_task greeter__build'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'runs package-local executables and restores their declared outputs from cache'
    command -v node >/dev/null 2>&1 || Skip 'node is required'
    mkdir -p "$WORKSPACE/packages/app/node_modules/.bin" "$WORKSPACE/node_modules/.bin"
    printf '#!/bin/sh\nprintf workspace\n' > "$WORKSPACE/node_modules/.bin/nx-probe"
    printf '#!/bin/sh\nprintf package > result.txt\n' > "$WORKSPACE/packages/app/node_modules/.bin/nx-probe"
    chmod +x "$WORKSPACE/node_modules/.bin/nx-probe" "$WORKSPACE/packages/app/node_modules/.bin/nx-probe"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "probe"
kind = "nx_task"
[target.attrs]
project = "app"
project_root = "packages/app"
task = "build"
runnable = true
executor = "nx:run-commands"
commands = ["nx-probe"]
command_cwd = "packages/app"
outputs = ["packages/app/result.txt"]
TOML

    When call rebuild_without_output
    The status should be success
    The stderr should be blank
    The stdout should include '"cache":"miss"'
    The stdout should include '"cache":"hit"'
    The contents of file "$WORKSPACE/packages/app/result.txt" should equal 'package'
  End

  It 'caches absent Nx outputs and restores existing output trees without hiding failures'
    command -v node >/dev/null 2>&1 || Skip 'node is required'
    mkdir -p "$WORKSPACE/packages/app"
    printf absent > "$WORKSPACE/packages/app/mode"
    cat > "$WORKSPACE/packages/app/probe.js" <<'JS'
const fs = require('node:fs');
const mode = fs.readFileSync('mode', 'utf8').trim();
if (mode === 'fail') process.exit(13);
fs.mkdirSync('node_modules/.vite/vitest', { recursive: true });
fs.writeFileSync('node_modules/.vite/vitest/results.json', String(Date.now()));
if (mode === 'present') {
  fs.mkdirSync('coverage', { recursive: true });
  fs.writeFileSync('coverage/result.txt', 'actual coverage');
}
JS
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "probe"
kind = "nx_task"
[target.attrs]
project = "app"
project_root = "packages/app"
task = "test"
runnable = true
executor = "nx:run-commands"
commands = ["node probe.js"]
command_cwd = "packages/app"
outputs = ["packages/app/coverage"]
TOML

    When call optional_outputs
    The status should be success
    The stderr should be blank
    The stdout should equal 'miss hit miss hit; failure propagated'
    The contents of file "$WORKSPACE/packages/app/coverage/result.txt" should equal 'actual coverage'
  End
End
