#!/usr/bin/env python3
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

report = json.loads(Path("once-tests.json").read_text())
missing = set()
for run in report.get("runs", []):
    if not run.get("success"):
        missing.update(re.findall(r"can't find crate for `([a-zA-Z_][a-zA-Z0-9_]*)`", run.get("stderr", "")))
search = [f"-Ldependency={path.resolve()}" for path in sorted(Path(".once/out").glob("*")) if path.is_dir()]
for name in sorted(missing):
    for artifact in sorted(Path(".once/out").glob(f"*/lib{name}-*.rlib")):
        print(f"Probing {name}: {artifact} ({artifact.stat().st_size} bytes)", flush=True)
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "probe.rs"
            source.write_text(f"extern crate {name};\n")
            subprocess.run([
                "rustc", str(source), "--crate-type", "rlib", "--emit", "metadata",
                "--extern", f"{name}={artifact.resolve()}", "--out-dir", directory, *search,
            ], env={**os.environ, "RUSTC_LOG": "rustc_metadata::locator=info"}, check=False)
