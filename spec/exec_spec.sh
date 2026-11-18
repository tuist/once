#shellcheck shell=bash
# End-to-end specs for `struo exec` (substrate escape hatch).

Describe 'struo exec'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_exec_workspace'

  cleanup_exec_workspace() {
    if [ -n "${DAYTONA_SERVER_PID:-}" ]; then
      kill "$DAYTONA_SERVER_PID" 2>/dev/null || true
      wait "$DAYTONA_SERVER_PID" 2>/dev/null || true
      unset DAYTONA_SERVER_PID
    fi
    cleanup_workspace
  }

  copy_exec_fixture() {
    cp -R "$REPO_ROOT/fixtures/exec/$1/." "$WORKSPACE/"
  }

  install_struo_shebangs() {
    for path in "$@"; do
      tmp="$path.tmp"
      sed "s|__STRUO_BIN__|$STRUO_BIN|g" "$path" > "$tmp"
      mv "$tmp" "$path"
      chmod +x "$path"
    done
  }

  It 'executes the command and reports a cache miss on first run'
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
    The status should be success
    The stdout should equal 'hello'
    The stderr should include 'cache miss'
    The stderr should include 'exit=0'
  End

  It 'records action evidence after execution'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf evidence' >/dev/null 2>&1
    When call struo --format json query evidence
    The status should be success
    The stdout should include '"schema":"struo.evidence.v1"'
    The stdout should include '"kind":"action_result"'
    The stdout should include '"status":"passed"'
    The stdout should include '"cache":"miss"'
    The stdout should include '"exit_code":0'
    The path "$WORKSPACE/.struo/struo.sqlite" should be file
  End

  It 'returns the cached result on a second identical invocation'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi' >/dev/null 2>&1
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi'
    The status should be success
    The stdout should equal 'hi'
    The stderr should include 'cache hit'
  End

  It 'does not re-execute the command on a cache hit'
    sentinel="$WORKSPACE/sentinel"
    cmd="touch '$sentinel'"
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c "$cmd" >/dev/null 2>&1
    rm -f "$sentinel"
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c "$cmd"
    The status should be success
    The stderr should include 'cache hit'
    The path "$sentinel" should not be exist
  End

  It 'executes an annotated script file directly through a Struo shebang'
    copy_exec_fixture direct_script
    install_struo_shebangs "$WORKSPACE/scripts/build.sh"
    printf 'hello direct script\n' > "$WORKSPACE/input.txt"

    When call env STRUO_CACHE_PROVIDER=local PATH=/usr/bin:/bin "$WORKSPACE/scripts/build.sh"
    The status should be success
    The stdout should equal 'hello direct script'
    The stderr should include 'cache miss'
  End

  It 'returns a cache hit when an annotated script file is re-executed directly'
    copy_exec_fixture direct_script
    install_struo_shebangs "$WORKSPACE/scripts/build.sh"
    printf 'cached direct script\n' > "$WORKSPACE/input.txt"

    env STRUO_CACHE_PROVIDER=local PATH=/usr/bin:/bin "$WORKSPACE/scripts/build.sh" >/dev/null 2>&1
    When call env STRUO_CACHE_PROVIDER=local PATH=/usr/bin:/bin "$WORKSPACE/scripts/build.sh"
    The status should be success
    The stdout should equal 'cached direct script'
    The stderr should include 'cache hit'
  End

  It 'executes an annotated script file through struo exec without --script'
    copy_exec_fixture annotated_script
    printf 'hello through struo exec\n' > "$WORKSPACE/input.txt"

    When call struo exec -e PATH=/usr/bin:/bin -- bash scripts/build.sh
    The status should be success
    The stdout should equal 'hello through struo exec'
    The stderr should include 'cache miss'
  End

  It 'executes an annotated script file through struo exec with --script'
    copy_exec_fixture annotated_script
    printf 'hello through struo exec --script\n' > "$WORKSPACE/input.txt"

    When call struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/build.sh
    The status should be success
    The stdout should equal 'hello through struo exec --script'
    The stderr should include 'cache miss'
  End

  It 'runs annotated scripts in an input sandbox with symlinked declared files'
    mkdir -p "$WORKSPACE/scripts"
    printf 'declared-input' > "$WORKSPACE/declared.txt"
    printf 'undeclared-input' > "$WORKSPACE/undeclared.txt"
    cat > "$WORKSPACE/scripts/build.sh" <<'SH'
#!/usr/bin/env bash
# struo input "../declared.txt"
# struo output "../out/result.txt"
set -e
test -L build.sh
test -L ../declared.txt
test ! -e ../undeclared.txt
mkdir -p ../out
cat ../declared.txt > ../out/result.txt
printf 'sandbox-ok'
SH

    When call struo exec --sandbox inputs --script -e PATH=/usr/bin:/bin -- bash scripts/build.sh
    The status should be success
    The stdout should equal 'sandbox-ok'
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/out/result.txt" should equal 'declared-input'
  End

  It 'runs needed scripts before an annotated script'
    copy_exec_fixture script_graph

    When call struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/package.sh
    The status should be success
    The stdout should equal 'generated:dependencyroot'
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/dist/app.txt" should equal 'generated:dependencyroot'
  End

  It 'uses needed script outputs in the dependent script cache key'
    copy_exec_fixture script_graph
    printf one > "$WORKSPACE/source.txt"
    printf root > "$WORKSPACE/main.txt"
    struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/package.sh >/dev/null 2>&1
    struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/package.sh >/dev/null 2>"$WORKSPACE/second.stderr"
    printf two > "$WORKSPACE/source.txt"

    When call struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/package.sh
    The status should be success
    The stdout should equal 'generated:tworoot'
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/second.stderr" should include 'cache hit'
  End

  It 'uses fingerprint command output in the script cache key'
    copy_exec_fixture fingerprint
    printf hello > "$WORKSPACE/input.txt"
    printf v1 > "$WORKSPACE/tool-version"
    struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/build.sh >/dev/null 2>&1
    struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/build.sh >/dev/null 2>"$WORKSPACE/second.stderr"
    printf v2 > "$WORKSPACE/tool-version"

    When call struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/build.sh
    The status should be success
    The stdout should equal 'hello'
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/second.stderr" should include 'cache hit'
  End

  It 'fails before running a script when a fingerprint command fails'
    copy_exec_fixture fingerprint_failure

    When call struo exec --script -e PATH=/usr/bin:/bin -- bash scripts/build.sh
    The status should not equal 0
    The stderr should include 'fingerprint command'
    The stderr should include 'probe-error'
    The path "$WORKSPACE/should-not-exist" should not be exist
  End

  It 'applies script timeouts to fingerprint commands'
    copy_exec_fixture fingerprint_timeout

    When call struo exec --script --timeout-ms 50 -e PATH=/usr/bin:/bin -- bash scripts/build.sh
    The status should not equal 0
    The stderr should include 'timed out after 50ms'
    The path "$WORKSPACE/should-not-exist" should not be exist
  End

  It 'runs a named local executor through the infrastructure binding'
    printf '[infrastructures.native]\nkind = "local"\n' > "$WORKSPACE/struo.toml"
    When call struo exec --remote --compute native -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf native-output'
    The status should be success
    The stdout should equal 'native-output'
    The stderr should include 'cache miss'
  End

  It 'resolves native tools and inputs for a script using a local executor alias'
    copy_exec_fixture annotated_script
    printf '[infrastructures.native]\nkind = "local"\n' > "$WORKSPACE/struo.toml"
    printf 'native-script-input\n' > "$WORKSPACE/input.txt"
    When call struo exec --remote --compute native -- bash scripts/build.sh
    The status should be success
    The stdout should equal 'native-script-input'
    The stderr should include 'cache miss'
  End

  It 'rejects network denial on an executor that cannot enforce it'
    When call struo exec --remote --compute e2b --network deny -- /bin/sh -c 'printf must-not-run'
    The status should be failure
    The stdout should equal ''
    The stderr should include 'does not support network denial'
  End

  It 'runs a command through the microsandbox compute provider'
    Skip if 'microsandbox specs are opt-in' microsandbox_specs_disabled
    When call "$STRUO_BIN" -C "$WORKSPACE" exec --remote --compute microsandbox -- /bin/sh -c 'printf remote-output'
    The status should be success
    The stdout should equal 'remote-output'
    The stderr should include 'cache miss'
  End

  It 'transfers only declared script inputs and retrieves microsandbox outputs'
    Skip if 'microsandbox specs are opt-in' microsandbox_specs_disabled
    copy_exec_fixture remote_dependency
    printf hidden > "$WORKSPACE/undeclared.txt"

    When call struo exec --script -- /bin/sh scripts/test.sh
    The status should be success
    The stdout should equal 'source:test'
    The stderr should include 'cache miss'
    The file "$WORKSPACE/reports/vitest.json" should be exist
    The contents of file "$WORKSPACE/reports/vitest.json" should include '"numPassedTests":1'
    The path "$WORKSPACE/node_modules/.bin/vitest" should be symlink
  End

  It 'restores remote dependency and test outputs from cache'
    Skip if 'microsandbox specs are opt-in' microsandbox_specs_disabled
    copy_exec_fixture remote_dependency
    printf hidden > "$WORKSPACE/undeclared.txt"
    if [ "${STRUO_RUN_MICROSANDBOX_SPECS:-}" = "1" ]; then
      struo exec --script -- /bin/sh scripts/test.sh >/dev/null 2>&1
      rm -rf "$WORKSPACE/node_modules" "$WORKSPACE/reports"
      printf changed > "$WORKSPACE/undeclared.txt"
    fi

    When call struo exec --script -- /bin/sh scripts/test.sh
    The status should be success
    The stdout should equal 'source:test'
    The stderr should include 'cache hit'
    The file "$WORKSPACE/reports/vitest.json" should be exist
    The contents of file "$WORKSPACE/reports/vitest.json" should include '"numPassedTests":1'
    The path "$WORKSPACE/node_modules/.bin/vitest" should be symlink
  End

  It 'runs a command through the daytona compute provider'
    copy_exec_fixture daytona
    start_python_server "$WORKSPACE/daytona_api.py" "$WORKSPACE/daytona_deleted" > "$WORKSPACE/daytona_port" &
    DAYTONA_SERVER_PID=$!
    wait_for_server_file "$WORKSPACE/daytona_port" "$DAYTONA_SERVER_PID" || return
    port="$(cat "$WORKSPACE/daytona_port")"
    When call env STRUO_DAYTONA_CONTROL_URL="http://127.0.0.1:$port/api" STRUO_DAYTONA_TOOLBOX_URL="http://127.0.0.1:$port/toolbox" STRUO_DAYTONA_API_KEY=token-1 "$STRUO_BIN" -C "$WORKSPACE" exec --remote --compute daytona -e VALUE=output -- /bin/sh -c 'printf "daytona-$VALUE"'
    The status should be success
    The stdout should equal 'daytona-output'
    The stderr should include 'cache miss'
    The file "$WORKSPACE/daytona_deleted" should be exist
  End

  It 'propagates the underlying exit code'
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 7'
    The status should equal 7
    The stderr should include 'exit=7'
  End

  It 'does not cache failing exits by default'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3' >/dev/null 2>&1 || true
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3'
    The status should equal 3
    # Second run is still a miss; failures are intentionally not
    # written to the cache without --cache-failures.
    The stderr should include 'cache miss'
  End

  It 'caches failing exits when --cache-failures is set'
    struo exec --cache-failures -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3' >/dev/null 2>&1 || true
    When call struo exec --cache-failures -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3'
    The status should equal 3
    The stderr should include 'cache hit'
  End

  It 'uses argv as part of the cache key'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf one' >/dev/null 2>&1
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf two'
    The status should be success
    The stdout should equal 'two'
    The stderr should include 'cache miss'
  End

  It 'uses env as part of the cache key'
    # printenv reads the env directly without going through a shell;
    # no quoting variability across bash/dash on different runners.
    struo exec -e PATH=/usr/bin:/bin -e MODE=a -- /usr/bin/printenv MODE >/dev/null 2>&1
    When call struo exec -e PATH=/usr/bin:/bin -e MODE=b -- /usr/bin/printenv MODE
    The status should be success
    The stdout should match pattern 'b*'
    The stderr should include 'cache miss'
  End

  It 'starts subprocesses with a cleared environment'
    export STRUO_PROBE=leaked
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c '[ -z "${STRUO_PROBE:-}" ]'
    The status should be success
    The stderr should include 'exit=0'
  End

  It 'creates the CAS directory under the XDG cache home'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'true' >/dev/null 2>&1
    When call test -d "$XDG_CACHE_HOME/struo/cas"
    The status should be success
  End

  It 'returns a cached result without rerunning the command'
    # Warm the cache with a deliberately slow command.
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 0.3; printf warmed' >/dev/null 2>&1
    When call struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 0.3; printf warmed'
    The status should be success
    The stdout should equal 'warmed'
    The stderr should include 'cache hit'
  End

  Describe 'cwd'
    It 'resolves a workspace-relative cwd against the workspace root'
      # Use `pwd` (POSIX shell builtin) so the assertion checks the
      # cwd resolution itself, without depending on file-write timing
      # or external utilities discoverable through PATH.
      mkdir -p "$WORKSPACE/sub"
      When call struo exec --cwd sub -e PATH=/usr/bin:/bin -- /bin/sh -c 'pwd'
      The status should be success
      The stdout should match pattern '*/sub*'
      The stderr should include 'cache miss'
    End

    It 'rejects an absolute cwd'
      When call struo exec --cwd /tmp -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
      The status should not equal 0
      The stderr should include 'workspace path must be relative'
    End

    It 'rejects a parent-directory escape'
      When call struo exec --cwd ../escape -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
      The status should not equal 0
      The stderr should include 'must not escape'
    End
  End

  Describe '--timeout-ms'
    It 'kills the child if the deadline elapses'
      When call struo exec --timeout-ms 100 -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 5'
      The status should not equal 0
      The stderr should include 'timeout'
    End
  End

  Describe '--format json'
    It 'emits a JSON trailer on stderr; stdout stays the wrapped program transparent'
      When call "$STRUO_BIN" --format json -C "$WORKSPACE" exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
      The status should be success
      # Stdout is the program's literal output, untouched.
      The stdout should equal 'hello'
      # The Struo trailer on stderr is now a JSON object.
      The stderr should include '"cache":"miss"'
      The stderr should include '"exit_code":0'
      The stderr should include '"action_digest":'
    End
  End

  Describe '--quiet'
    It 'suppresses the human trailer on stderr but keeps subprocess stdout'
      When call "$STRUO_BIN" --quiet -C "$WORKSPACE" exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
      The status should be success
      # Wrapped program stdout still passes through untouched.
      The stdout should equal 'hello'
      # The "struo: cache ..." trailer must not appear under --quiet.
      The stderr should not include 'cache miss'
      The stderr should not include 'struo:'
    End

    It 'still emits the structured trailer when --quiet is combined with --format json'
      # --quiet only gates human trailers; machine consumers asked
      # for json get json regardless. Stdout remains the wrapped
      # program's bytes so existing pipelines stay parseable.
      When call "$STRUO_BIN" --quiet --format json -C "$WORKSPACE" exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
      The status should be success
      The stdout should equal 'hello'
      The stderr should include '"cache":"miss"'
      The stderr should include '"exit_code":0'
    End

    It 'still surfaces errors when --quiet is set'
      # Errors travel to stderr through a different code path than the
      # success trailer; --quiet must not swallow them.
      When call "$STRUO_BIN" --quiet -C /nonexistent exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
      The status should not equal 0
      The stderr should include 'struo:'
    End

    It 'accepts the short -q alias'
      When call "$STRUO_BIN" -q -C "$WORKSPACE" exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi'
      The status should be success
      The stdout should equal 'hi'
      The stderr should not include 'cache miss'
    End
  End

  Describe '--format toon'
    It 'emits a TOON trailer on stderr; stdout stays the wrapped program transparent'
      When call "$STRUO_BIN" --format toon -C "$WORKSPACE" exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
      The status should be success
      The stdout should equal 'hello'
      The stderr should include 'cache: miss'
      The stderr should include 'exit_code: 0'
      The stderr should include 'action_digest:'
    End
  End

  Describe '--verify-reproducible'
    It 'compares artifacts separately from logs without changing existing outputs'
      cat > "$WORKSPACE/build.sh" <<'SH'
#!/usr/bin/env -S struo exec --script /bin/sh
# struo output "out.txt"
test ! -e out.txt || exit 9
printf fresh > out.txt
printf '%s' "$$"
SH
      printf original > "$WORKSPACE/out.txt"
      When call struo --format json exec --verify-reproducible -- ./build.sh
      The status should be success
      The stdout should include '"artifacts_match":true'
      The stdout should include '"logs_match":false'
      The stdout should include 'struo query verification'
      The contents of file "$WORKSPACE/out.txt" should equal 'original'
    End

    It 'retains differing dependency outputs for inspection and reruns dependencies'
      cat > "$WORKSPACE/generate.sh" <<'SH'
#!/bin/sh
# struo output "generated.txt"
printf 'prefix:%s' "$$" > generated.txt
SH
      cat > "$WORKSPACE/build.sh" <<'SH'
#!/bin/sh
# struo needs "generate.sh"
# struo input "generated.txt"
# struo output "result.txt"
cp generated.txt result.txt
SH
      verify_and_inspect() {
        struo --format json exec --verify-reproducible -- /bin/sh build.sh > "$SPEC_ROOT/report.json"
        status=$?
        [ "$status" -eq 1 ] || return 90
        id=$(jq -r .id "$SPEC_ROOT/report.json")
        struo --format json query verification "$id" > "$SPEC_ROOT/retained.json" || return
        cmp "$SPEC_ROOT/report.json" "$SPEC_ROOT/retained.json" || return
        for trial in 1 2; do
          grep -q '^prefix:' "$WORKSPACE/.struo/verifications/$id/trial-$trial/outputs/result.txt" || return
        done
        expected_offset=$(cmp -l "$WORKSPACE/.struo/verifications/$id/trial-1/outputs/result.txt" "$WORKSPACE/.struo/verifications/$id/trial-2/outputs/result.txt" 2>/dev/null | awk 'NR == 1 { print $1 - 1; exit }')
        [ "$(jq -r '.artifact_differences[0].first_differing_byte' "$SPEC_ROOT/retained.json")" = "$expected_offset" ] || return
        jq -c '[.success, .execution_succeeded, .artifacts_match, .artifact_differences[0].path, .artifact_differences[0].kind]' "$SPEC_ROOT/retained.json"
      }
      When call verify_and_inspect
      The status should be success
      The stdout should equal '[false,true,false,"result.txt","content"]'
      The path "$WORKSPACE/result.txt" should not be exist
      The path "$WORKSPACE/generated.txt" should not be exist
    End

    It 'restores declared inputs that are updated in place for each trial'
      cat > "$WORKSPACE/update.sh" <<'SH'
#!/bin/sh
# struo input "state.txt"
# struo output "state.txt"
printf ':updated' >> state.txt
SH
      printf original > "$WORKSPACE/state.txt"
      When call struo --format json exec --verify-reproducible -- /bin/sh update.sh
      The status should be success
      The stdout should include '"artifacts_match":true'
      The contents of file "$WORKSPACE/state.txt" should equal 'original'
    End

    It 'distinguishes permission changes from changed artifact bytes'
      cat > "$WORKSPACE/build.sh" <<'SH'
#!/bin/sh
# struo output "out.txt"
printf stable > out.txt
if test -e "$COUNTER"; then
  chmod 600 out.txt
else
  chmod 644 out.txt
  touch "$COUNTER"
fi
SH
      When call struo --format json exec --verify-reproducible -e "COUNTER=$SPEC_ROOT/counter" -- /bin/sh build.sh
      The status should be failure
      The stdout should include '"artifacts_match":false'
      The stdout should include '"kind":"permissions"'
      The stdout should include '"first_differing_byte":null'
    End

    It 'does not call two identical failures a successful verification'
      When call struo --format json exec --verify-reproducible -- /bin/sh -c 'printf failure >&2; exit 7'
      The status should be failure
      The stdout should include '"execution_succeeded":false'
      The stdout should include '"artifacts_match":null'
      The stdout should include '"exit_code":7'
      The stdout should include '"success":false'
    End

    It 'propagates network denial to annotated dependencies or refuses execution'
      printf '#!%s\n' "$SPEC_PYTHON3" > "$WORKSPACE/dependency.py"
      cat >> "$WORKSPACE/dependency.py" <<'PY'
# struo output "dependency.txt"
import errno
import pathlib
import socket
try:
    socket.create_connection(("127.0.0.1", 1), timeout=1).close()
    status = "allowed"
except OSError as error:
    status = "denied" if error.errno in (errno.EACCES, errno.EPERM) else "allowed"
pathlib.Path("dependency.txt").write_text(status)
PY
      cat > "$WORKSPACE/build.sh" <<'SH'
#!/bin/sh
# struo needs "dependency.py"
# struo input "dependency.txt"
# struo output "out.txt"
cp dependency.txt out.txt
SH
      denied_or_refused() {
        struo exec --network deny -- /bin/sh build.sh > "$SPEC_ROOT/stdout" 2> "$SPEC_ROOT/stderr"
        if [ "$?" -eq 0 ]; then
          [ "$(cat "$WORKSPACE/out.txt")" = denied ] || return
        else
          grep -q 'network denial' "$SPEC_ROOT/stderr" || return
          [ ! -e "$WORKSPACE/dependency.txt" ] || return
        fi
        printf blocked
      }
      When call denied_or_refused
      The status should be success
      The stdout should equal 'blocked'
    End

    It 'rejects offline fingerprints instead of running them without the policy'
      cat > "$WORKSPACE/build.sh" <<'SH'
#!/bin/sh
# struo fingerprint "touch fingerprint-ran"
# struo output "out.txt"
printf must-not-run > out.txt
SH
      When call struo exec --network deny -- /bin/sh build.sh
      The status should be failure
      The stderr should include 'fingerprint commands cannot enforce --network deny'
      The path "$WORKSPACE/fingerprint-ran" should not be exist
    End
  End

  Describe 'large outputs'
    big_exec() {
      "$STRUO_BIN" -C "$WORKSPACE" exec --timeout-ms 10000 \
        -e PATH=/usr/bin:/bin -- /bin/sh -c 'yes hello | head -c 4194304' \
        > "$WORKSPACE/big.out" 2>/dev/null
      wc -c "$WORKSPACE/big.out" | awk '{print $1}'
    }

    It 'streams multi-megabyte output without buffering everything'
      # Run via a helper so the 4 MB stream stays in a real file and
      # shellspec only sees the byte count on stdout.
      When call big_exec
      The status should be success
      The output should equal '4194304'
    End
  End
End
