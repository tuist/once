#shellcheck shell=bash
# End-to-end specs for the graph Once derives from an Nx workspace without a
# manifest of its own. Nx's project graph API provides the graph that Once
# translates into task targets so it can schedule and cache them.

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
    The stdout should include 'nx_test hello_world__test'
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

  generated_dependency_changes() {
    SEED=first once --format json build consumer >/dev/null || return
    test "$(cat "$WORKSPACE/result.txt")" = first || return
    SEED=second once --format json build consumer >/dev/null || return
    test "$(cat "$WORKSPACE/result.txt")" = second || return
    rm "$WORKSPACE/generated.txt" "$WORKSPACE/result.txt"
    SEED=second once --format json build consumer >/dev/null || return
    test "$(cat "$WORKSPACE/generated.txt")" = second || return
    cat "$WORKSPACE/result.txt"
  }

  It 'invalidates consumers when generated dependency contents change without source changes'
    command -v node >/dev/null 2>&1 || Skip 'node is required'
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "generate"
kind = "nx_task"
[target.attrs]
project = "app"
task = "generate"
runnable = true
executor = "nx:run-commands"
commands = ["printf %s \"$SEED\" > generated.txt"]
env_inherit = ["SEED"]
outputs = ["generated.txt"]
workspace_outputs = ["generated.txt", "result.txt"]

[[target]]
name = "consumer"
kind = "nx_task"
deps = ["generate"]
[target.attrs]
project = "app"
task = "build"
runnable = true
executor = "nx:run-commands"
commands = ["cp generated.txt result.txt"]
outputs = ["result.txt"]
workspace_outputs = ["generated.txt", "result.txt"]
TOML

    When call generated_dependency_changes
    The status should be success
    The stderr should be blank
    The stdout should equal 'second'
  End

  It 'does not skip failing test prerequisites of builds'
    command -v node >/dev/null 2>&1 || Skip 'node is required'
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "check"
kind = "nx_test"
[target.attrs]
project = "app"
task = "test"
runnable = true
executor = "nx:run-commands"
commands = ["exit 7"]

[[target]]
name = "consumer"
kind = "nx_task"
deps = ["check"]
[target.attrs]
project = "app"
task = "build"
runnable = true
executor = "nx:run-commands"
commands = ["printf built > result.txt"]
outputs = ["result.txt"]
TOML

    When call once build consumer
    The status should be failure
    The stderr should include 'exit code 7'
    The path "$WORKSPACE/result.txt" should not be exist
  End

  It 'runs plugin executors with native JSONC workspace configuration'
    nx_toolchain_unavailable && Skip 'node and npm are required'
    printf '%s\n' '{"private":true,"devDependencies":{"nx":"^23.0.0","@nx/devkit":"^23.0.0"}}' > "$WORKSPACE/package.json"
    install_fixture || Skip 'npm install failed; skipping (offline?)'
    printf '%s\n' '{/* native Nx accepts comments */ "parallel":1,}' > "$WORKSPACE/nx.json"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "probe"
kind = "nx_task"
[target.attrs]
project = "app"
task = "build"
runnable = true
executor = "nx:noop"
executor_graph = '{"nodes":{"app":{"name":"app","type":"app","data":{"root":".","targets":{"build":{"executor":"nx:noop"}}}}},"dependencies":{}}'
TOML

    When call once --format json build probe
    The status should be success
    The stderr should be blank
    The stdout should include '"status":"completed"'
  End

  compiler_dependency_outputs() {
    result=$(once --format json build app__build) || return
    printf '%s\n' "$result" | jq -er '.cache == "miss"' >/dev/null || return
    rm -rf "$WORKSPACE/dist"
    result=$(once --format json build app__build) || return
    printf '%s\n' "$result" | jq -er '.cache == "hit"' >/dev/null || return
    (cd "$WORKSPACE" && node -e 'console.log(require("./dist/app/index.js").answer, require("./dist/app/package.json").dependencies.tslib)')
  }

  It 'retains external package dependencies for Nx 17 compiler actions and cache restoration'
    nx_toolchain_unavailable && Skip 'node and npm are required'
    cat > "$WORKSPACE/package.json" <<'JSON'
{"name":"nx17-probe","private":true,"dependencies":{"tslib":"2.4.1"},"devDependencies":{"nx":"17.1.3","@nx/js":"17.1.3","typescript":"5.2.2"}}
JSON
    install_fixture || Skip 'npm install failed; skipping (offline?)'
    printf '%s\n' '{"plugins":["./dotenv-plugin.cjs"]}' > "$WORKSPACE/nx.json"
    printf '%s\n' 'ONCE_NX_GRAPH_BASE=loaded' 'ONCE_NX_GRAPH_VALUE=${ONCE_NX_GRAPH_BASE}-local' > "$WORKSPACE/.local.env"
    printf '%s\n' 'ONCE_NX_GRAPH_VALUE=wrong' > "$WORKSPACE/.env.local"
    cat > "$WORKSPACE/dotenv-plugin.cjs" <<'JS'
module.exports = {
  name: 'dotenv-probe',
  createNodes: ['**/project.json', () => {
    if (process.env.ONCE_NX_GRAPH_VALUE !== 'loaded-local') {
      throw new Error('root dotenv precedence and expansion were not preserved');
    }
    return { projects: {} };
  }],
};
JS
    printf '%s\n' '{"compilerOptions":{"importHelpers":true,"declaration":true,"module":"commonjs","target":"es2018","types":[]}}' > "$WORKSPACE/tsconfig.base.json"
    mkdir -p "$WORKSPACE/packages/app/src"
    printf '%s\n' 'export const answer: number = 42;' > "$WORKSPACE/packages/app/src/index.ts"
    printf '%s\n' '{"extends":"../../tsconfig.base.json","include":["src/**/*.ts"]}' > "$WORKSPACE/packages/app/tsconfig.lib.json"
    cat > "$WORKSPACE/packages/app/project.json" <<'JSON'
{"name":"app","sourceRoot":"packages/app/src","projectType":"library","targets":{"build":{"executor":"@nx/js:tsc","outputs":["{options.outputPath}"],"options":{"outputPath":"dist/app","main":"packages/app/src/index.ts","rootDir":"packages/app/src","tsConfig":"packages/app/tsconfig.lib.json","assets":[],"updateBuildableProjectDepsInPackageJson":true}}}}
JSON

    When call compiler_dependency_outputs
    The status should be success
    The stderr should be blank
    The stdout should equal '42 2.4.1'
  End
End
