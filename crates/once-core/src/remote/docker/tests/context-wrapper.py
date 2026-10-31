#!/usr/bin/env python3
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent
binary = os.environ["ONCE_DOCKER_REAL_BIN"]
args = sys.argv[1:]
operation = args[:]
while operation and operation[0] in ("--config", "--context"):
    operation = operation[2:]
if operation[0] == "rm" and not (root / "allow-delete").exists():
    if (root / "switch").exists():
        subprocess.run(
            [binary, "--config", str(root / "config"), "context", "use", "empty"],
            check=True,
            stdout=sys.stderr,
        )
    sys.exit(1)
if operation[0] == "create":
    result = subprocess.run([binary] + args, stdout=subprocess.PIPE)
    if result.returncode == 0:
        (root / "container-id").write_bytes(result.stdout)
    sys.stdout.buffer.write(result.stdout)
    sys.exit(result.returncode)
if operation[0] == "ps":
    with (root / "listings").open("a") as log:
        log.write(repr(args) + "\n")
os.execv(binary, [binary] + args)
