#!/usr/bin/env python3

import argparse
import datetime
import json
import os
import signal
import shutil
import subprocess
import threading
import time
from pathlib import Path


def process_rows(include_command=False):
    fields = "pid=,ppid=,rss=,command=" if include_command else "pid=,ppid=,rss="
    output = subprocess.run(
        ["ps", "-axo", fields],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    rows = {}
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=3)
        if len(fields) < 3:
            continue
        process_id, parent_id, resident_kibibytes = fields[:3]
        rows[int(process_id)] = {
            "parent": int(parent_id),
            "resident_kibibytes": int(resident_kibibytes),
            "command": fields[3] if len(fields) == 4 else "",
        }
    return rows


def selected_processes(rows, root_process, additional_roots):
    selected = {root_process, *additional_roots}
    changed = True
    while changed:
        changed = False
        for process_id, row in rows.items():
            if process_id not in selected and row["parent"] in selected:
                selected.add(process_id)
                changed = True
    selected.discard(os.getpid())
    return selected


def measure(
    label,
    command,
    environment,
    working_directory,
    log_path,
    command_marker,
    sample_resident_memory,
):
    log_path.parent.mkdir(parents=True, exist_ok=True)
    additional_roots = set()
    if sample_resident_memory and command_marker:
        command_rows = process_rows(include_command=True)
        additional_roots.update(
            process_id
            for process_id, row in command_rows.items()
            if command_marker in row["command"] and row["parent"] == 1
        )
    start = time.monotonic_ns()
    memory = {"peak_resident_kibibytes": None}
    with log_path.open("w") as log:
        process = subprocess.Popen(
            command,
            cwd=working_directory,
            env=environment,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            text=True,
        )
        stop_sampling = threading.Event()

        def sample_memory():
            while not stop_sampling.is_set():
                rows = process_rows()
                selected = selected_processes(rows, process.pid, additional_roots)
                resident_kibibytes = sum(
                    rows[process_id]["resident_kibibytes"]
                    for process_id in selected
                    if process_id in rows
                )
                memory["peak_resident_kibibytes"] = max(
                    memory["peak_resident_kibibytes"],
                    resident_kibibytes,
                )
                stop_sampling.wait(0.2)

        sampler = None
        if sample_resident_memory:
            memory["peak_resident_kibibytes"] = 0
            sampler = threading.Thread(target=sample_memory, daemon=True)
            sampler.start()
        try:
            return_code = process.wait()
            finish = time.monotonic_ns()
        except BaseException:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait()
            raise
        finally:
            stop_sampling.set()
            if sampler is not None:
                sampler.join()
    elapsed_seconds = (finish - start) / 1_000_000_000
    result = {
        "label": label,
        "elapsed_seconds": elapsed_seconds,
        "peak_resident_kibibytes": memory["peak_resident_kibibytes"],
        "return_code": return_code,
        "command": command,
        "log": str(log_path),
        "measured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }
    if return_code != 0:
        raise subprocess.CalledProcessError(return_code, command)
    return result


def reset_once(checkout, cache_home):
    shutil.rmtree(checkout / ".once", ignore_errors=True)
    shutil.rmtree(cache_home / "once" / "cas", ignore_errors=True)


def mise_environment(tool):
    raw = subprocess.run(
        ["mise", "exec", tool, "--", "env", "-0"],
        check=True,
        capture_output=True,
    ).stdout
    return {
        name.decode(): value.decode()
        for entry in raw.split(b"\0")
        if entry
        for name, value in [entry.split(b"=", 1)]
    }


def mise_executable(tool, name):
    root = subprocess.run(
        ["mise", "where", tool],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    executable = Path(root) / name
    if not executable.is_file():
        raise RuntimeError(f"missing {name} executable under {root}")
    return str(executable)


def bazel_command(checkout, output_base, disk_cache, prepared_launcher):
    command = (
        [mise_executable("bazel@9.0.0", "bazel")]
        if prepared_launcher
        else ["mise", "exec", "bazel@9.0.0", "--", "bazel"]
    )
    return [
        *command,
        f"--output_base={output_base}",
        "build",
        f"--disk_cache={disk_cache}",
        "//codex-rs/cli:codex",
    ]


def reset_bazel(checkout, output_base, disk_cache):
    if output_base.exists():
        subprocess.run(
            [
                "mise",
                "exec",
                "bazel@9.0.0",
                "--",
                "bazel",
                f"--output_base={output_base}",
                "shutdown",
            ],
            cwd=checkout,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    shutil.rmtree(output_base, ignore_errors=True)
    shutil.rmtree(disk_cache, ignore_errors=True)


def verify_checkout(checkout, expected_revision):
    revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=checkout,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if revision != expected_revision:
        raise RuntimeError(
            f"expected Codex revision {expected_revision}, found {revision}"
        )


def append_result(path, result):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as output:
        output.write(json.dumps(result, sort_keys=True) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("system", choices=["once", "bazel"])
    parser.add_argument("temperature", choices=["cold", "warm"])
    parser.add_argument("--checkout", type=Path, required=True)
    parser.add_argument("--once", type=Path)
    parser.add_argument("--once-cache-home", type=Path)
    parser.add_argument("--bazel-output-base", type=Path)
    parser.add_argument("--bazel-disk-cache", type=Path)
    parser.add_argument("--state-root", type=Path, required=True)
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--log-directory", type=Path, required=True)
    parser.add_argument(
        "--sample-memory",
        action="store_true",
        help="sample aggregate process-tree memory in a background thread",
    )
    parser.add_argument(
        "--prepared-launcher",
        action="store_true",
        help="prepare tool paths and environments before the timed process",
    )
    parser.add_argument(
        "--revision",
        default="3947f0d0c3e255bade02e241c16cb43d284c0e65",
    )
    args = parser.parse_args()

    checkout = args.checkout.resolve()
    state_root = args.state_root.resolve()
    verify_checkout(checkout, args.revision)
    state_root.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)

    if args.system == "once":
        if args.once is None:
            parser.error("--once is required for a Once measurement")
        cache_home = (
            args.once_cache_home.resolve()
            if args.once_cache_home is not None
            else state_root / "once-cache-home"
        )
        environment["XDG_CACHE_HOME"] = str(cache_home)
        if args.temperature == "cold":
            if args.once_cache_home is not None:
                parser.error(
                    "--once-cache-home cannot be combined with a cold reset"
                )
            reset_once(checkout, cache_home)
        if args.prepared_launcher:
            environment = mise_environment("rust@1.95.0")
            environment["XDG_CACHE_HOME"] = str(cache_home)
            command = [str(args.once.resolve())]
        else:
            command = [
                "mise",
                "exec",
                "rust@1.95.0",
                "--",
                str(args.once.resolve()),
            ]
        command.extend([
            "build",
            "-C",
            str(checkout),
            "codex",
            "--format",
            "json",
        ])
        command_marker = None
    else:
        output_base = (
            args.bazel_output_base.resolve()
            if args.bazel_output_base is not None
            else state_root / "bazel-output-base"
        )
        disk_cache = (
            args.bazel_disk_cache.resolve()
            if args.bazel_disk_cache is not None
            else state_root / "bazel-disk-cache"
        )
        if args.temperature == "cold":
            if args.bazel_output_base is not None or args.bazel_disk_cache is not None:
                parser.error(
                    "existing Bazel state cannot be combined with a cold reset"
                )
            reset_bazel(checkout, output_base, disk_cache)
        command = bazel_command(
            checkout,
            output_base,
            disk_cache,
            args.prepared_launcher,
        )
        command_marker = str(output_base)

    label = f"{args.system}-{args.temperature}"
    timestamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    result = measure(
        label,
        command,
        environment,
        checkout,
        args.log_directory.resolve() / f"{timestamp}-{label}.log",
        command_marker,
        args.sample_memory,
    )
    result["revision"] = args.revision
    append_result(args.results.resolve(), result)
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
