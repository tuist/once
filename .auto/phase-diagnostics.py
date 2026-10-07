#!/usr/bin/env python3
import datetime
import heapq
import json
import os
from pathlib import Path
import statistics
import subprocess
import time

root = Path(__file__).resolve().parent.parent
fixture = root / "benchmarks/cache-comparison/once"
environment = dict(os.environ)
environment.update(
    TUIST_TOKEN="benchmark",
    XDG_CACHE_HOME=str(root / "benchmarks/cache-comparison/.state/once-client/cache"),
    XDG_CONFIG_HOME=str(root / "benchmarks/cache-comparison/.state/once-client/config"),
)
commands = {
    "wrapper": [str(root / "benchmarks/cache-comparison/run-once.sh")],
    "direct": [str(root / "target/release/once"), "build", "distribution", "--format", "json", "--quiet"],
}
measurements = {name: [] for name in commands}
started = time.time()
for iteration in range(12):
    order = list(commands) if iteration % 2 == 0 else list(reversed(commands))
    for name in order:
        before = time.perf_counter()
        subprocess.run(commands[name], cwd=fixture, env=environment, stdout=subprocess.DEVNULL, check=True)
        measurements[name].append((time.perf_counter() - before) * 1000)

report = {name + "_median_ms": statistics.median(values) for name, values in measurements.items()}
report["wrapper_overhead_ms"] = report["wrapper_median_ms"] - report["direct_median_ms"]

if os.uname().sysname == "Darwin":
    logs = Path.home() / "Library/Logs/Once"
else:
    logs = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))) / "once/logs"
phases = {"session_ms": [], "preflight_ms": [], "receipt_ms": [], "parse_ms": [], "logging_ms": [], "discovery_client_ms": [], "discovery_request_ms": []}
for path in heapq.nlargest(80, logs.glob("*.log")):
    if path.stat().st_mtime < started:
        continue
    records = [json.loads(line) for line in path.read_text().splitlines()]
    messages = {record["fields"].get("message", ""): record for record in records}
    if "reused unchanged build receipt" not in messages:
        continue
    if messages["reused unchanged build receipt"]["fields"].get("target") != "distribution":
        continue
    def stamp(message):
        return datetime.datetime.fromisoformat(messages[message]["timestamp"].replace("Z", "+00:00")).timestamp()
    if "session started" in messages and "session finished" in messages:
        phases["session_ms"].append((stamp("session finished") - stamp("session started")) * 1000)
        startup = messages["session started"]["fields"]
        for source, target in [("parse_us", "parse_ms"), ("logging_us", "logging_ms")]:
            if source in startup:
                phases[target].append(float(startup[source]) / 1000)
    if "discovery request completed" in messages:
        discovery = messages["discovery request completed"]["fields"]
        phases["discovery_client_ms"].append(float(discovery["client_init_us"]) / 1000)
        phases["discovery_request_ms"].append(float(discovery["request_us"]) / 1000)
    if "Once live reporter: no events endpoint configured" in messages:
        preflight = stamp("Once live reporter: no events endpoint configured")
        phases["preflight_ms"].append((preflight - stamp("session started")) * 1000)
        phases["receipt_ms"].append((stamp("reused unchanged build receipt") - preflight) * 1000)
for name, values in phases.items():
    if values:
        report[name] = statistics.median(values)
report["phase_samples"] = len(phases["session_ms"])
print("DIAGNOSTIC " + json.dumps(report, sort_keys=True))
