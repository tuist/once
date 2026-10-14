import functools
import hashlib
import http.server
import io
import json
import pathlib
import sys
import tarfile


workspace = pathlib.Path(sys.argv[1])
sources = pathlib.Path(sys.argv[2])
registry = workspace / ".once" / "registry-fixture"
crate = registry / "crates" / "itoa" / "1.0.14" / "download"
crate.parent.mkdir(parents=True)
manifest = b'[package]\nname = "itoa"\nversion = "1.0.14"\nedition = "2018"\n'
with tarfile.open(crate, "w:gz") as archive:
    info = tarfile.TarInfo("itoa-1.0.14/Cargo.toml")
    info.size = len(manifest)
    archive.addfile(info, io.BytesIO(manifest))
    archive.add(sources / "src", arcname="itoa-1.0.14/src")

checksum = hashlib.sha256()
with crate.open("rb") as contents:
    for chunk in iter(lambda: contents.read(65536), b""):
        checksum.update(chunk)
checksum = checksum.hexdigest()
index = registry / "it" / "oa" / "itoa"
index.parent.mkdir(parents=True)
index.write_text(json.dumps({
    "name": "itoa", "vers": "1.0.14", "deps": [], "features": {},
    "cksum": checksum, "yanked": False,
}) + "\n")

handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(registry))
server = http.server.HTTPServer(("127.0.0.1", 0), handler)
url = "http://127.0.0.1:" + str(server.server_port)
(registry / "config.json").write_text(json.dumps({
    "dl": url + "/crates/{crate}/{version}/download",
}))
(workspace / ".cargo" / "config.toml").write_text(
    '[source.crates-io]\nreplace-with = "test-registry"\n'
    '[source.test-registry]\nregistry = "sparse+' + url + '/"\n'
)
lockfile = workspace / "Cargo.lock"
lockfile.write_text(lockfile.read_text().replace(
    "d75a2a4b1b190afb6f5425f10f6a8f959d2ea0b9c2b1d79553551850539e4674", checksum,
))
(registry / "ready").write_text("ready\n")
server.serve_forever()
