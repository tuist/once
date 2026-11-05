"""Exercise the compiler recorder with real rustc, without building Once."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import parity


class ComparisonTest(unittest.TestCase):
    def test_later_compilation_does_not_hide_an_empty_build(self):
        unit = '["fixture", "src/lib.rs", false]'
        native = {
            "build_exit": 0,
            "build_units": [unit],
            "test_exit": 0,
            "units": {unit: {"exit": 0}},
            "tests": {"same test binary": {"exit": 0}},
            "executed_cases": [("same case", "ok")],
        }
        once = native | {"build_units": []}
        self.assertEqual(
            parity.compare(native, once),
            [{"field": "build_units", "native": native["build_units"], "once": []}],
        )


class CommandBoundaryTest(unittest.TestCase):
    def test_inherited_remote_provider_is_overridden(self):
        with patch.dict(os.environ, {"ONCE_CACHE_PROVIDER": "remote"}):
            self.assertEqual(parity.clean_environment()["ONCE_CACHE_PROVIDER"], "local")

    @unittest.skipUnless(sys.platform == "linux", "requires Linux procps pkill")
    def test_timeout_stops_descendants(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child = (
                "from pathlib import Path; import os, time; os.setpgrp(); "
                "Path('ready').touch(); time.sleep(3); Path('survived').touch()"
            )
            parent = (
                "import subprocess, sys, time; "
                f"subprocess.Popen([sys.executable, '-c', {child!r}]); time.sleep(10)"
            )
            with self.assertRaises(subprocess.TimeoutExpired):
                parity.run(
                    [sys.executable, "-c", parent],
                    root,
                    os.environ,
                    root / "command.log",
                    timeout=2,
                )
            self.assertTrue((root / "ready").exists())
            time.sleep(3)
            self.assertFalse((root / "survived").exists())


class CompilerObservationTest(unittest.TestCase):
    def test_codegen_spellings_and_last_wins(self):
        for flags, expected in (
            (
                [
                    "-Copt-level=1",
                    "--codegen",
                    "opt-level=3",
                    "--codegen=opt-level=2",
                    "-C",
                    "debuginfo=1",
                    "--codegen=debuginfo=2",
                    "--codegen",
                    "embed-bitcode=no",
                ],
                {"opt-level": "2", "debuginfo": "2", "embed-bitcode": "no"},
            ),
            (
                [
                    "--codegen=opt-level=3",
                    "-C",
                    "opt-level=1",
                    "--codegen=debuginfo=2",
                    "-Cdebuginfo=1",
                    "--codegen=panic=abort",
                ],
                {"opt-level": "1", "debuginfo": "1", "panic": "abort"},
            ),
        ):
            with self.subTest(flags=flags), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                logs = root / "logs"
                logs.mkdir()
                (root / "input.rs").write_text("pub fn value() -> u32 { 7 }\n")
                subprocess.run(
                    [
                        sys.executable,
                        str(Path(__file__).with_name("parity.py")),
                        "_rustc",
                        shutil.which("rustc"),
                        str(logs),
                        str(root),
                        str(root),
                        "--crate-name",
                        "flags",
                        "--crate-type=lib",
                        "--emit=metadata",
                        "-o",
                        str(root / "output.rmeta"),
                        *flags,
                        "input.rs",
                    ],
                    cwd=root,
                    env=os.environ | {"CARGO_PKG_NAME": "flags"},
                    check=True,
                )
                records = list(logs.glob("*.json"))
                self.assertEqual(len(records), 1)
                record = json.loads(records[0].read_text())
                self.assertEqual(record["exit"], 0)
                self.assertEqual(record["sources"], ["input.rs"])
                for key, value in expected.items():
                    self.assertEqual(record["codegen"][key], value)


if __name__ == "__main__":
    unittest.main()
