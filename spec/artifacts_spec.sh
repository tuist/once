#shellcheck shell=bash

Describe 'artifact export and publication'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  apple_toolchain_unavailable() {
    [ "$(uname -s)" != "Darwin" ] || ! command -v xcrun >/dev/null 2>&1
  }

  android_toolchain_unavailable() {
    [ ! -f "${ANDROID_HOME:-}/platforms/android-35/android.jar" ] || ! command -v keytool >/dev/null 2>&1
  }

  inspect_android_export() {
    python3 - "$ONCE_BIN" "$WORKSPACE" "$REPO_ROOT" <<'PY'
import hashlib, json, os, pathlib, shutil, struct, subprocess, sys, zlib
binary, workspace, repo = sys.argv[1:]
root = pathlib.Path(workspace)
shutil.copytree(pathlib.Path(repo) / 'crates/once-frontend/prelude/examples/android-binary-minimal', root, dirs_exist_ok=True)
(root / 'once.toml').write_text('')
keystore = pathlib.Path.home() / '.android/debug.keystore'
keystore.parent.mkdir(parents=True, exist_ok=True)
subprocess.run(['keytool', '-genkeypair', '-keystore', str(keystore), '-storepass', 'android', '-keypass', 'android', '-alias', 'androiddebugkey', '-dname', 'CN=Artifact Test', '-keyalg', 'RSA', '-validity', '365', '-storetype', 'JKS'], check=True, capture_output=True)
target = 'apps/hello/Hello'
def run(*args):
    result = subprocess.run([binary, '-C', workspace, '--format', 'json', *args], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)
def png(size):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = b''.join(b'\x00' + bytes([88, 64, 200, 255]) * size for _ in range(size))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b'')
def exported(result, icon = None):
    directory = pathlib.Path(result['directory']) / 'blobs/sha256'
    def read(descriptor):
        path = directory / descriptor['digest'].split(':')[1]
        data = path.read_bytes()
        assert len(data) == descriptor['size_bytes']
        assert 'sha256:' + hashlib.sha256(data).hexdigest() == descriptor['digest']
        return path
    index = json.loads(read(result['root']).read_bytes())
    manifest = json.loads(read(index['members']['product']['artifact']).read_bytes())
    spec = manifest['interfaces']['dev.once.android-install.v1']['spec']
    if icon is None:
        assert 'icon' not in manifest['payloads']
        assert spec == {'metadata_payload': 'metadata'}
    else:
        assert spec == {'metadata_payload': 'metadata', 'icon_payload': 'icon'}
        assert manifest['payloads']['icon']['media_type'] == 'image/png'
        data = read(manifest['payloads']['icon']).read_bytes()
        assert data[:8] == b'\x89PNG\r\n\x1a\n' and data[12:16] == b'IHDR'
        assert struct.unpack('>II', data[16:24]) == (icon, icon)
    return read(manifest['payloads']['apk']), json.loads(read(manifest['payloads']['metadata']).read_bytes())
run('build', target)
first = run('artifact', 'export', target)
apk, metadata = exported(first)
tools = pathlib.Path(os.environ['ANDROID_HOME']) / 'build-tools/35.0.0'
subprocess.run([str(tools / 'apksigner'), 'verify', str(apk)], check=True, capture_output=True)
assert metadata['application_id'] == 'dev.once.hello' and metadata['display_name'] == 'Once Hello'
assert metadata['version_code'] == '1' and metadata['version_name'] == '1.0'
assert metadata['minimum_sdk'] == ['23']
assert metadata['signature_verified'] is True and metadata['certificate_sha256']
assert first['automatic_publication'] is True
shutil.rmtree(root / '.once/out')
assert run('artifact', 'export', target)['root'] == first['root']
launcher = root / 'apps/hello/res/mipmap-xhdpi/ic_launcher.png'
launcher.parent.mkdir(parents=True)
launcher.write_bytes(png(96))
android_manifest = root / 'apps/hello/AndroidManifest.xml'
android_manifest.write_text(android_manifest.read_text().replace('android:label=', 'android:icon="@mipmap/ic_launcher"\n        android:label=', 1))
with_icon = run('artifact', 'export', target)
exported(with_icon, icon = 96)
shutil.rmtree(root / '.once/out')
assert run('artifact', 'export', target)['root'] == with_icon['root']
manifest = root / 'apps/hello/once.toml'
manifest.write_text(manifest.read_text() + '\nsigning = "none"\n')
unsigned = run('artifact', 'export', target)
_, metadata = exported(unsigned, icon = 96)
assert metadata['signature_verified'] is False and metadata['certificate_sha256'] == []
assert unsigned['automatic_publication'] is False
assert unsigned['root'] != first['root']
assert not (root / '.once/publications').exists()
print('native signed APK, metadata, icon, cache replay and unsigned exclusion verified')
PY
  }

  inspect_apple_export() {
    python3 - "$ONCE_BIN" "$WORKSPACE" "$REPO_ROOT" <<'PY'
import hashlib, json, pathlib, platform, plistlib, shutil, subprocess, sys
binary, workspace, repo = sys.argv[1:]
root = pathlib.Path(workspace)
shutil.copytree(pathlib.Path(repo) / 'crates/once-frontend/prelude/examples/apple-application-macos', root, dirs_exist_ok=True)
def run(*args):
    return json.loads(subprocess.check_output([binary, '-C', workspace, '--format', 'json', *args]))
run('build', 'Hello')
assert not list((root / '.once/out').rglob('*.zip'))
first = run('artifact', 'export', 'Hello')
directory = pathlib.Path(first['directory'])
def content(descriptor):
    path = directory / 'blobs/sha256' / descriptor['digest'].split(':')[1]
    data = path.read_bytes()
    assert len(data) == descriptor['size_bytes']
    assert 'sha256:' + hashlib.sha256(data).hexdigest() == descriptor['digest']
    return path, data
_, index = content(first['root'])
member = json.loads(index)['members']['product']
assert member['selectors'] == {'platform': 'macos', 'sdk_variant': 'device', 'architecture': platform.machine()}
_, manifest = content(member['artifact'])
manifest = json.loads(manifest)
archive, _ = content(manifest['payloads']['bundle'])
_, metadata = content(manifest['payloads']['metadata'])
metadata = json.loads(metadata)
destination = root / 'unpacked'
subprocess.run(['/usr/bin/ditto', '-x', '-k', str(archive), str(destination)], check=True)
app = destination / 'Hello.app'
subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)], check=True)
plist = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
assert metadata['bundle_identifier'] == plist['CFBundleIdentifier'] == 'dev.once.Hello'
assert metadata['platform'] == 'macos' and metadata['sdk_variant'] == 'device'
assert metadata['signature_verified'] is True
assert manifest['interfaces']['dev.once.apple-install.v2']['spec'] == {'metadata_payload': 'metadata'}
assert 'icon' not in manifest['payloads']
executable = app / 'Contents/MacOS' / plist['CFBundleExecutable']
uuid = subprocess.check_output(['xcrun', 'dwarfdump', '--uuid', str(executable)], text=True).split()[1]
assert metadata['mach_o_uuids'] == [uuid]
assert first['automatic_publication'] is True
assert executable.stat().st_mode & 0o111
assert run('artifact', 'export', 'Hello')['root'] == first['root']
assert not (root / '.once/publications').exists()
print('signed Apple bundle, native metadata, cache replay and explicit publication verified')
PY
  }

  inspect_local_publication() {
    python3 - "$ONCE_BIN" "$WORKSPACE" "$REPO_ROOT" "$XDG_CACHE_HOME" <<'PY'
import hashlib, json, pathlib, shutil, subprocess, sys, zipfile
binary, workspace, repo, cache = sys.argv[1:]
root = pathlib.Path(workspace)
shutil.copytree(pathlib.Path(repo) / 'crates/once-frontend/prelude/examples/artifact-static-site', root, dirs_exist_ok=True)
def run(*args):
    return json.loads(subprocess.check_output([binary, '-C', workspace, '--format', 'json', *args]))
contract = run('query', 'artifacts', 'site')
assert contract['supported'] is True
first = run('artifact', 'export', 'site')
shutil.rmtree(root / '.once/out')
second = run('artifact', 'export', 'site')
assert first['root'] == second['root']
one = run('artifact', 'publish', 'site', '--provider', 'local', '--track', 'one')
two = run('artifact', 'publish', 'site', '--provider', 'local', '--track', 'two')
assert one['artifact'] == two['artifact'] == first['root']
assert one['track'] == 'one' and two['track'] == 'two'
assert len(list((root / '.once/publications').glob('*.json'))) == 2
directory = pathlib.Path(first['directory'])
def content(descriptor):
    data = (directory / 'blobs/sha256' / descriptor['digest'].split(':')[1]).read_bytes()
    assert len(data) == descriptor['size_bytes']
    assert 'sha256:' + hashlib.sha256(data).hexdigest() == descriptor['digest']
    return data
index = json.loads(content(first['root']))
manifest = json.loads(content(index['members']['product']['artifact']))
assert manifest['interfaces']['dev.once.static-http.v1']['spec']['root'] == 'public'
content(manifest['payloads']['content'])
archive = directory / 'blobs/sha256' / manifest['payloads']['content']['digest'].split(':')[1]
with zipfile.ZipFile(archive) as zipped:
    assert zipped.namelist() == ['public/index.html']
    assert b'A shareable build' in zipped.read('public/index.html')
shutil.rmtree(cache)
shutil.rmtree(root / '.once/out')
assert archive.is_file()
requests = [
    {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/list', 'params': {}},
    {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {'name': 'once_publish_artifacts', 'arguments': {'target': 'site', 'provider': 'local'}}},
]
process = subprocess.run([binary, '-C', workspace, 'mcp'], input=''.join(json.dumps(r) + '\n' for r in requests), text=True, capture_output=True, check=True)
responses = [json.loads(line) for line in process.stdout.splitlines()]
names = [tool['name'] for tool in responses[0]['result']['tools']]
assert 'once_query_artifacts' in names and 'once_publish_artifacts' not in names
assert responses[1]['result']['isError'] is True
print('portable content, cache replay, independent retention and explicit publication verified')
PY
  }

  inspect_native_names_and_links() {
    python3 - "$ONCE_BIN" "$WORKSPACE" <<'PY'
import json, pathlib, stat, subprocess, sys, zipfile
binary, workspace = sys.argv[1:]
root = pathlib.Path(workspace)
(root / 'data').mkdir()
(root / 'data/a').write_text('alpha')
(root / 'data/b').write_text('beta')
(root / 'alias').symlink_to('data/a')
(root / 'linked').symlink_to('data', target_is_directory=True)
def export(name):
    (root / 'once.toml').write_text('[[target]]\nname = ' + json.dumps(name, ensure_ascii=False) + '\nkind = "artifact"\nsrcs = ["data/**/*", "alias", "linked"]\n')
    result = json.loads(subprocess.check_output([binary, '-C', workspace, '--format', 'json', 'artifact', 'export', name]))
    directory = pathlib.Path(result['directory']) / 'blobs/sha256'
    def content(descriptor):
        return directory / descriptor['digest'].split(':')[1]
    index = json.loads(content(result['root']).read_bytes())
    manifest = json.loads(content(index['members']['product']['artifact']).read_bytes())
    return result['root'], content(manifest['payloads']['content'])
first = None
for name in ('My App', '冰块', 'n' * 140):
    identity, archive = export(name)
    if first is None: first = identity
    assert identity == first
    with zipfile.ZipFile(archive) as zipped:
        assert zipped.namelist() == ['alias', 'data/a', 'data/b', 'linked']
        for path, target in [('alias', b'data/a'), ('linked', b'data')]:
            assert stat.S_ISLNK(zipped.getinfo(path).external_attr >> 16)
            assert zipped.read(path) == target
        assert zipped.read('data/a') == b'alpha'
(root / 'alias').unlink()
(root / 'alias').symlink_to('data/b')
changed, archive = export('My App')
assert changed != first
with zipfile.ZipFile(archive) as zipped:
    assert zipped.read('alias') == b'data/b'
assert export('My App')[0] == changed
print('native names, file and directory links, retargeting and cache replay verified')
PY
  }

  inspect_publication_service() {
    python3 - "$ONCE_BIN" "$WORKSPACE" <<'PY'
import hashlib, http.server, json, os, pathlib, subprocess, sys, threading, traceback, urllib.parse
binary, workspace = sys.argv[1:]
root = pathlib.Path(workspace)
(root / 'modules').mkdir()
(root / 'modules/product.star').write_text('''
def _impl(ctx):
    out = declare_output("upload.bin")
    copy_path("upload.bin", out)
    metadata = declare_output("metadata.json")
    write_path(metadata, '{"example":"opaque metadata"}')
    return {"artifact_info": _artifact_info([{"name": name, "payloads": {"content": {"path": out, "media_type": "application/octet-stream"}, "metadata": {"path": metadata, "media_type": "application/json"}}, "interfaces": {"example.future.v7": {"payload": "content", "spec": {"metadata_payload": "metadata"}}}} for name in ["one", "two"]], automatic_publication = True)}
product = target_kind(impl = _impl, providers = ["once_artifact_info"], capabilities = [capability("build", ["default"]), capability("artifact", ["default"])])
''')
payload = b'a' * (8 * 1024 * 1024) + b'final-part-123'
(root / 'upload.bin').write_bytes(payload)
state = {'mode': 'ok', 'contents': {}, 'publications': {}, 'errors': [], 'calls': []}
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        try:
            self.path = urllib.parse.unquote(self.path)
            assert self.path.startswith('/api/projects/acme/app/artifacts'), self.path
            assert self.headers['Authorization'] == 'Bearer fixture-token'
            state['calls'].append(('GET', self.path))
            if state['mode'] == 'unauthorized': return self.reply(401, {})
            if self.path.endswith('/artifacts'):
                destination = 'project:other' if state['mode'] == 'changed-destination' else 'project:123'
                return self.reply(200, {'schema': 'once.publication_service.v1', 'destination': destination})
            key = self.path.rsplit('/', 1)[1]
            if key not in state['publications']: return self.reply(404, {})
            receipt = dict(state['publications'][key])
            if state['mode'] == 'wrong-destination': receipt['destination'] = 'project:other'
            if state['mode'] == 'bad-url': receipt['url'] = state['receipt-url']
            return self.reply(200, receipt)
        except Exception as error:
            state['errors'].append(traceback.format_exc())
            self.reply(400, {})
    def do_POST(self):
        try:
            self.path = urllib.parse.unquote(self.path)
            assert self.headers['Authorization'] == 'Bearer fixture-token'
            body = self.rfile.read(int(self.headers['Content-Length']))
            state['calls'].append(('POST', self.path))
            if '/content/' in self.path:
                digest = self.path.rsplit('/', 1)[1]
                assert digest == 'sha256:' + hashlib.sha256(body).hexdigest()
                if state['mode'] == 'upload-failed': return self.reply(500, {})
                if state['mode'] == 'conflict': return self.reply(409, {})
                state['contents'][digest] = body
                return self.reply(204, {})
            intent = json.loads(body)
            key = self.path.rsplit('/', 1)[1]
            assert key == 'sha256:' + hashlib.sha256(json.dumps(intent, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
            def content(descriptor):
                data = state['contents'][descriptor['digest']]
                assert len(data) == descriptor['size_bytes']
                return data
            index = json.loads(content(intent['root']))
            assert set(index['members']) == {'one', 'two'}
            for member in index['members'].values():
                manifest = json.loads(content(member['artifact']))
                assert content(manifest['payloads']['content']) == payload
                assert json.loads(content(manifest['payloads']['metadata'])) == {'example': 'opaque metadata'}
            if state['mode'] == 'commit-failed': return self.reply(500, {})
            receipt = {'schema': 'once.publication_receipt.v1', 'key': key, 'root': intent['root'], 'destination': 'project:123', 'url': f'http://127.0.0.1:{self.server.server_port}/publication'}
            state['publications'][key] = receipt
            if state['mode'] == 'unconfirmed':
                self.close_connection = True
                return
            return self.reply(200, receipt)
        except Exception as error:
            state['errors'].append(traceback.format_exc())
            self.reply(400, {})
server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
manifest = f'''[modules]
paths = ["modules/*.star"]
[infrastructures.fixture]
kind = "tuist"
url = "http://127.0.0.1:{server.server_port}"
account = "acme"
project = "app"
[infrastructure.artifacts]
name = "fixture"
[[target]]
name = "product"
kind = "product"
srcs = ["upload.bin"]
'''
(root / 'once.toml').write_text(manifest)
def run(*args, success=True, cwd=None, input=None):
    process = subprocess.run([binary, '-C', workspace, '--format', 'json', *args], cwd=cwd, input=input, env={**os.environ, 'TUIST_TOKEN': 'fixture-token'}, text=True, capture_output=True)
    assert not state['errors'], state['errors']
    assert (process.returncode == 0) == success, (args, process.stdout, process.stderr)
    return process
try:
    run('build', 'product')
    assert state['calls'] == []
    enabled = manifest.replace('[infrastructure.artifacts]', '[infrastructure]\npublish_on_build = ["product"]\n[infrastructure.artifacts]')
    (root / 'once.toml').write_text(enabled)
    run('build', 'product')
    assert state['calls'] == []
    state['calls'].clear()
    run('artifact', 'publish', 'product', '--allow-on-build')
    assert len(state['publications']) == 1 and len(state['contents']) == 4
    state['calls'].clear()
    requests = [{'jsonrpc': '2.0', 'id': index, 'method': 'tools/call', 'params': {'name': 'once_build_target', 'arguments': {'target': 'product'}}} for index in (1, 2)]
    mcp = run('mcp', '--allow-run', input=''.join(json.dumps(request) + '\n' for request in requests))
    responses = [json.loads(line)['result']['structuredContent']['result'] for line in mcp.stdout.splitlines()]
    assert len(responses) == 2
    for response in responses:
        assert response['success'] is True, response
        assert response['record_parse_error'] is None, response
        assert response['record']['target'] == 'product' and response['record']['capability'] == 'build'
    assert len(state['calls']) == 4 and all(method == 'GET' for method, _ in state['calls'])
    state['calls'].clear()
    assert json.loads(run('build', 'product').stdout)['target'] == 'product'
    assert all(method == 'GET' for method, _ in state['calls']) and len(state['calls']) == 2
    (root / 'nested').mkdir()
    (root / 'once.toml').write_text(enabled.replace('["product"]', '["./product"]'))
    state['calls'].clear()
    run('build', 'product', cwd=root / 'nested')
    assert len(state['calls']) == 2 and all(method == 'GET' for method, _ in state['calls'])
    (root / 'once.toml').write_text(enabled.replace(f':{server.server_port}"', f':{server.server_port}/changed"'))
    state['calls'].clear()
    run('build', 'product')
    assert state['calls'] == []
    (root / 'once.toml').write_text(enabled)
    state.update(mode='changed-destination', calls=[])
    run('build', 'product', success=False)
    assert len(state['calls']) == 1 and state['calls'][0][0] == 'GET'
    state.update(mode='ok', calls=[])
    import shutil
    shutil.rmtree(root / '.once/out')
    run('build', 'product')
    assert len(state['publications']) == 1
    (root / 'once.toml').write_text(manifest)
    state['calls'].clear()
    run('build', 'product')
    assert state['calls'] == []
    (root / 'once.toml').write_text(enabled)
    module = root / 'modules/product.star'
    source = module.read_text()
    module.write_text(source.replace('automatic_publication = True', 'automatic_publication = False'))
    run('build', 'product')
    assert state['calls'] == []
    module.write_text(source)
    for mode in ('unauthorized', 'upload-failed', 'commit-failed', 'conflict', 'unconfirmed', 'wrong-destination'):
        state.update(mode=mode, calls=[])
        args = ['artifact', 'publish', 'product']
        if mode != 'wrong-destination': args += ['--track', mode]
        receipts = list((root / '.once/publications').glob('*.json'))
        run(*args, success=False)
        assert list((root / '.once/publications').glob('*.json')) == receipts
        if mode == 'unconfirmed':
            count = len(state['publications'])
            state.update(mode='ok', calls=[])
            run(*args)
            assert len(state['publications']) == count
            assert all(method == 'GET' for method, _ in state['calls'])
    for url in ('', '/relative', 'javascript:alert(1)', 'http://elsewhere.invalid/app', 'https://user:secret@example.invalid/app'):
        state.update(mode='bad-url', **{'receipt-url': url})
        receipts = list((root / '.once/publications').glob('*.json'))
        run('artifact', 'publish', 'product', success=False)
        assert list((root / '.once/publications').glob('*.json')) == receipts
    state.update(mode='upload-failed', calls=[])
    (root / 'upload.bin').write_bytes(payload + b'changed')
    receipts = list((root / '.once/publications').glob('*.json'))
    run('build', 'product', success=False)
    assert list((root / '.once/publications').glob('*.json')) == receipts
    state.update(mode='ok', calls=[])
    module.write_text(source + '''
def _broken(ctx):
    fail("deliberate build failure")
broken = target_kind(impl = _broken, capabilities = [capability("build", ["default"])])
''')
    (root / 'once.toml').write_text(enabled + '\n[[target]]\nname = "zzbroken"\nkind = "broken"\n')
    failed = run('build', '--all', success=False)
    assert 'deliberate build failure' in failed.stderr
    assert state['calls'] == []
finally:
    server.shutdown()
    server.server_close()
    thread.join()
print('full closure, consent, cache replay, disabled publication and failure semantics verified')
PY
  }

  It 'retains verified content across cache replay and publishes only explicitly'
    When call inspect_local_publication
    The status should be success
    The stdout should include 'explicit publication verified'
  End

  It 'exports native target names and preserves selected links across retargeting'
    Skip if 'requires symbolic links' test "$(uname -s)" = "Windows_NT"
    When call inspect_native_names_and_links
    The status should be success
    The stdout should include 'retargeting and cache replay verified'
  End

  It 'publishes complete artifact sets with consent and idempotent confirmation'
    When call inspect_publication_service
    The status should be success
    The stdout should include 'failure semantics verified'
  End

  It 'exports a signed Apple application with matching native preview metadata'
    Skip if 'requires the Apple toolchain' apple_toolchain_unavailable
    When call inspect_apple_export
    The status should be success
    The stdout should include 'explicit publication verified'
  End

  It 'exports verified Android applications and excludes unsigned outputs from automation'
    Skip if 'requires the Android toolchain' android_toolchain_unavailable
    When call inspect_android_export
    The status should be success
    The stdout should include 'unsigned exclusion verified'
  End
End
