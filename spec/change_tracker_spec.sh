#shellcheck shell=bash

Describe 'filesystem change tracker process'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  inspect_tracker_process() {
    "$SPEC_PYTHON3" - "$ONCE_BIN" "$WORKSPACE" <<'PY'
import fcntl, json, os, pathlib, resource, select, shutil, socket, subprocess, sys, time, uuid
binary, workspace = sys.argv[1:]
root = pathlib.Path(workspace)
original_binary = binary
binary = str(root.parent / 'replaceable-once')
shutil.copy2(original_binary, binary)
# Keep the Unix socket path below the platform limit without copying its hash algorithm.
while True:
    runtime = pathlib.Path('/tmp') / uuid.uuid4().hex[:4]
    try:
        runtime.mkdir()
        break
    except FileExistsError:
        pass
env = {**os.environ, 'XDG_RUNTIME_DIR': str(runtime), 'ONCE_CHANGE_TRACKER': '1', 'ONCE_CACHE_PROVIDER': 'local'}
(root / 'modules').mkdir()
(root / 'modules/probe.star').write_text('''
def _impl(ctx):
    out = declare_output("product")
    run_action(argv = ["/bin/cp", "input.txt", out], inputs = ["input.txt"], outputs = [out], create_dirs = [ctx["build_dir"]])
    return {}
probe = target_kind(impl = _impl, capabilities = [capability("build", ["default"])])
''')
manifest = root / 'once.toml'
manifest.write_text('[modules]\npaths = ["modules/*.star"]\n[[target]]\nname = "app"\nkind = "probe"\nsrcs = ["input.txt"]\n')
source = root / 'input.txt'
source.write_text('first')
output = root / '.once/out/app/product'
socket_path = None

def run(*args, **options):
    result = subprocess.run([binary, '-C', workspace, '--memory-limit', '1GiB', '--format', 'json', *args], env=env, text=True, capture_output=True, timeout=45, **options)
    assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result

def snapshot(since=None):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(12)
        client.connect(str(socket_path))
        client.sendall(json.dumps({'command': 'barrier', 'outputs': ['.once/out/app/product'], 'since': since}).encode() + b'\n')
        client.shutdown(socket.SHUT_WR)
        with client.makefile('rb') as stream:
            response = json.loads(stream.readline(1024 * 1024))
        assert 'error' not in response, response
        return response['snapshot']

def changed(since, area, paths):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        result = snapshot(since)
        changes = result[area + '_changes']
        if changes is None or any(path in changes for path in paths):
            assert result['position'][area + '_generation'] > since[area + '_generation'], result
            return result['position']
        time.sleep(0.05)
    raise AssertionError(('unobserved mutation', area, paths, result))

try:
    read_fd, write_fd = os.pipe()
    os.set_inheritable(write_fd, True)
    high_fd = None
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    try:
        if hard == resource.RLIM_INFINITY or hard > 5000:
            resource.setrlimit(resource.RLIMIT_NOFILE, (max(soft, 5001), hard))
            high_fd = fcntl.fcntl(write_fd, fcntl.F_DUPFD, 5000)
            resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))
        descriptors = (write_fd,) if high_fd is None else (write_fd, high_fd)
        assert all(os.get_inheritable(fd) for fd in descriptors)
        # Reduce only the build child's limit after inheriting an actual high descriptor.
        reduced_soft = 256 if soft == resource.RLIM_INFINITY else min(soft, 256)
        run('build', 'app', pass_fds=descriptors, preexec_fn=lambda: resource.setrlimit(resource.RLIMIT_NOFILE, (reduced_soft, hard)))
    finally:
        resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))
        os.close(write_fd)
        if high_fd is not None:
            os.close(high_fd)
    try:
        assert select.select([read_fd], [], [], 5)[0], 'daemon kept an inherited pipe open'
        assert os.read(read_fd, 1) == b'', 'expected EOF with the daemon still alive'
    finally:
        os.close(read_fd)
    sockets = list(runtime.rglob('*.sock'))
    assert len(sockets) == 1, ('build did not start its tracker', sockets)
    socket_path = sockets[0]
    assert output.read_text() == 'first'
    previous = snapshot()['position']
    for _ in range(20):
        current = snapshot(previous)
        if current['position'] == previous:
            break
        previous = current['position']
    else:
        raise AssertionError('tracker did not settle')
    for _ in range(3):
        assert manifest.read_text()
        assert source.read_text() == 'first'
        assert output.read_text() == 'first'
        list(root.iterdir())
        current = snapshot(previous)
        assert current['position'] == previous, ('read-only access invalidated files', current)
        assert current['source_changes'] == [] and current['output_changes'] == [], current
    run('build', 'app')
    current = snapshot(previous)
    assert current['position'] == previous, ('no-op build invalidated files', current)

    source.write_text('second')
    previous = changed(previous, 'source', ['input.txt'])
    renamed = root / 'renamed.txt'
    source.rename(renamed)
    previous = changed(previous, 'source', ['input.txt', 'renamed.txt'])
    renamed.rename(source)
    previous = changed(previous, 'source', ['input.txt', 'renamed.txt'])
    source.chmod(source.stat().st_mode ^ 0o100)
    previous = changed(previous, 'source', ['input.txt'])
    created = root / 'created.txt'
    created.write_text('created')
    previous = changed(previous, 'source', ['created.txt'])
    created.unlink()
    previous = changed(previous, 'source', ['created.txt'])
    run('build', 'app')
    assert output.read_text() == 'second', 'source invalidation did not reach the build'
    previous = snapshot()['position']
    output.write_text('tampered')
    previous = changed(previous, 'output', ['.once/out/app/product'])
    run('build', 'app')
    assert output.read_text() == 'second', 'output invalidation did not restore cached bytes'
    literal = run('exec', '--', sys.executable, '-c', 'import sys; print(sys.argv[1])', '__change-tracker')
    assert '__change-tracker' in literal.stdout, literal
    # The private launcher lets the user-facing executable be replaced while the daemon lives.
    shutil.copyfile(original_binary, binary)
    run('build', 'app')
    print('daemon startup, inherited pipe EOF, read-only stability, mutations and barriers verified')
    print('high descriptor above reduced soft limit verified' if high_fd is not None else 'high descriptor unavailable under hard limit')
finally:
    failed = sys.exc_info()[0] is not None
    shutil.rmtree(root, ignore_errors=True)
    try:
        if socket_path is not None:
            deadline = time.monotonic() + 5
            while socket_path.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            if socket_path.exists():
                if not failed:
                    raise AssertionError('daemon did not exit after workspace removal')
                print('cleanup: daemon did not exit after workspace removal', file=sys.stderr)
    finally:
        shutil.rmtree(runtime)
PY
  }

  It 'starts a detached tracker without retaining inherited pipes or invalidating reads'
    Skip if 'requires Unix descriptors and filesystem notifications' test "$(uname -s)" = "Windows_NT"
    When call inspect_tracker_process
    The status should be success
    The stdout should include 'daemon startup, inherited pipe EOF, read-only stability, mutations and barriers verified'
  End
End
