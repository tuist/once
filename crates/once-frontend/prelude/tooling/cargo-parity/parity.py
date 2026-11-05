#!/usr/bin/env python3
"""Bounded Cargo differential experiment, not a general semantic equivalence checker."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
TRIPLE = "x86_64-unknown-linux-gnu"
SEEDS = ("feature", "source", "build-dependency", "generated-file", "test")


def run(argv, cwd, env, log, timeout=180):
    with (
        log.open("w") as output,
        subprocess.Popen(
            argv,
            cwd=cwd,
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        ) as process,
    ):
        try:
            result = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            subprocess.run(
                ["pkill", "-KILL", "--session", str(process.pid)],
                timeout=5,
                check=False,
            )
            process.wait(timeout=5)
            raise
    if log.stat().st_size > 4 * 1024 * 1024:
        raise RuntimeError(f"observation exceeds fixture log budget: {log}")
    return result


def values(args, option):
    result = []
    for index, arg in enumerate(args):
        if arg == option:
            result.append(args[index + 1])
        elif arg.startswith(option + "="):
            result.append(arg[len(option) + 1 :])
    return result


def compiler(real, logs, proxy, workspace, args):
    if args == ["--print", "sysroot"]:
        print(proxy)
        return 0
    roots = [arg for arg in args if arg.endswith(".rs") and Path(arg).is_file()]
    if not roots or "--print" in args:
        return subprocess.call([real, *args])
    record = {
        "argv": args,
        "cwd": str(Path.cwd()),
        "root": roots[-1],
        "env": {
            key: value
            for key, value in os.environ.items()
            if key in ("CARGO_PKG_NAME", "CARGO_MANIFEST_DIR", "OUT_DIR", "E7_MARKER")
        },
    }
    record["exit"] = subprocess.call([real, *args])
    manifest = Path(record["env"].get("CARGO_MANIFEST_DIR", workspace)).resolve()
    out = Path(record["env"].get("OUT_DIR", "/nonexistent")).resolve()
    root = Path(roots[-1]).resolve()
    record["unit"] = [
        record["env"].get("CARGO_PKG_NAME", "UNKNOWN")
        if "--crate-name" in args
        else "UNKNOWN",
        root.relative_to(manifest).as_posix()
        if root.is_relative_to(manifest)
        else "UNMATCHED:" + str(root),
        "--test" in args,
    ]
    record["externs"] = sorted(
        value.split("=")[0] for value in values(args, "--extern")
    )
    cfg = subprocess.run(
        [real, *args, "--print", "cfg"],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    record["cfg"] = (
        sorted(cfg.stdout.splitlines()) if cfg.returncode == 0 else ["UNKNOWN"]
    )
    codegen = {}
    for index, arg in enumerate(args):
        if arg in ("-C", "--codegen"):
            option = args[index + 1]
        elif arg.startswith("-C"):
            option = arg[2:]
        elif arg.startswith("--codegen="):
            option = arg[len("--codegen=") :]
        else:
            continue
        if "=" in option:
            key, value = option.split("=", 1)
            codegen[key] = value
    record["codegen"] = {
        key: codegen.get(key, default)
        for key, default in (
            ("opt-level", "0"),
            ("debuginfo", "0"),
            ("embed-bitcode", "yes"),
            ("panic", "unwind"),
        )
    }
    record["edition"] = values(args, "--edition")
    record["target"] = values(args, "--target") or [TRIPLE]
    record["crate_type"] = [
        "rlib" if value == "lib" else value
        for value in (values(args, "--crate-type") or ["bin"])
    ]
    record["sources"] = None
    record["generated"] = None
    if record["exit"] == 0:
        record["sources"] = []
        record["generated"] = {}
        depfile = Path(logs) / (uuid.uuid4().hex + ".d")
        dep_args = []
        skip = False
        for arg in args:
            if skip:
                skip = False
                continue
            if arg in ("--emit", "-o", "--out-dir"):
                skip = True
            elif not arg.startswith(("--emit=", "--out-dir=")):
                dep_args.append(arg)
        dep = subprocess.run(
            [real, *dep_args, "--emit=dep-info=" + str(depfile)],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        if dep.returncode:
            raise RuntimeError("dep-info observation failed: " + dep.stderr)
        # These controlled fixtures have no whitespace or escaped Make paths.
        dependencies = depfile.read_text().splitlines()[0].split(": ", 1)[1].split()
        for source in dependencies:
            path = Path(source).resolve()
            if path.is_relative_to(out):
                if path.stat().st_size > 1024 * 1024:
                    raise RuntimeError("generated input exceeds fixture budget")
                record["generated"][path.relative_to(out).as_posix()] = {
                    "sha256": file_digest(path),
                    "text": path.read_text(),
                }
            elif path.is_relative_to(manifest):
                record["sources"].append(path.relative_to(manifest).as_posix())
            else:
                record["sources"].append("UNMATCHED:" + str(path))
        record["sources"].sort()
        depfile.unlink()
    if "--test" in args and record["exit"] == 0:
        outputs = values(args, "-o")
        if outputs:
            executable = Path(outputs[-1]).resolve()
        else:
            extra = codegen.get("extra-filename", "")
            executable = Path(values(args, "--out-dir")[-1]) / (
                values(args, "--crate-name")[-1] + extra
            )
        record["executable"] = str(executable)
    (Path(logs) / (uuid.uuid4().hex + ".json")).write_text(
        json.dumps(record, indent=2) + "\n"
    )
    return record["exit"]


def seed_error(root, seed):
    def replace(path, before, after):
        file = root / path
        text = file.read_text()
        assert text.count(before) == 1, (path, before)
        file.write_text(text.replace(before, after))

    if seed == "feature":
        replace("once.toml", 'features = ["extra"]', "features = []")
    elif seed == "source":
        replace(
            "src/lib.rs", "mod selected;", "mod selected { pub const VALUE: u32 = 7; }"
        )
    elif seed == "build-dependency":
        replace(
            "Cargo.toml",
            '[build-dependencies]\ngenerator = { path = "deps/generator" }\n',
            "",
        )
        replace(
            "build.rs",
            'generator::generate(include_str!("schema.txt"))',
            'include_str!("schema.txt").trim().parse::<u32>().unwrap()',
        )
    elif seed == "generated-file":
        replace("build.rs", 'out.join("generated.rs")', 'out.join("wrong.rs")')
    elif seed == "test":
        replace("Cargo.toml", 'edition = "2021"', 'edition = "2021"\nautotests = false')


def clean_environment():
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(
            ("CARGO_PROFILE_", "CARGO_BUILD_", "CARGO_TARGET_")
        ) or key in (
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTDOCFLAGS",
            "RUSTC_BOOTSTRAP",
        ):
            del env[key]
    env.update(
        CARGO_INCREMENTAL="0",
        CARGO_TERM_COLOR="never",
        ONCE_CACHE_PROVIDER="local",
        NO_COLOR="1",
        RUST_TEST_THREADS="1",
        MISE_AUTO_INSTALL="false",
    )
    return env


def observe(base, fixture, arm, seed, once, real, environment):
    directory = base / (fixture + "-" + arm + "-" + (seed or "control"))
    root = directory / "workspace"
    shutil.copytree(HERE / "fixtures" / fixture, root)
    logs = directory / "compiler"
    logs.mkdir()
    proxy = directory / "sysroot"
    (proxy / "bin").mkdir(parents=True)
    real_sysroot = Path(
        subprocess.check_output([real, "--print", "sysroot"], text=True).strip()
    )
    (proxy / "lib").symlink_to(real_sysroot / "lib", target_is_directory=True)
    (proxy / "bin" / "rustdoc").symlink_to(real_sysroot / "bin" / "rustdoc")
    shim = proxy / "bin" / "rustc"
    shim.write_text(
        f"#!{sys.executable}\nimport os\nos.execv({sys.executable!r}, "
        f"{[sys.executable, str(Path(__file__).resolve()), '_rustc', real, str(logs), str(proxy), str(root)]!r} + __import__('sys').argv[1:])\n"
    )
    shim.chmod(0o755)
    env = environment | {
        "PATH": str(proxy / "bin") + os.pathsep + environment["PATH"],
        "RUSTC": str(shim),
    }
    # A private Cargo home prevents user configuration from silently changing the reference.
    env["CARGO_HOME"] = str(directory / "cargo-home")
    if arm == "once" and seed:
        seed_error(root, seed)
    lock_exit = run(
        ["cargo", "generate-lockfile", "--offline"], root, env, directory / "lock.log"
    )
    if lock_exit:
        raise RuntimeError("fixture lock generation failed")
    shutil.copy2(root / "Cargo.lock", directory / "Cargo.lock")
    common = [
        "--locked",
        "--offline",
        "--no-default-features",
        "--features",
        "extra",
        "--target",
        TRIPLE,
        "--profile",
        "dev",
        "-j",
        "1",
    ]
    if arm == "native":
        build = run(["cargo", "build", *common], root, env, directory / "build.log")
    else:
        build = run(
            [once, "--memory-limit", "2560MiB", "--format", "json", "build", "cargo"],
            root,
            env,
            directory / "build.log",
        )
    build_units = {
        json.dumps(record["unit"])
        for path in logs.glob("*.json")
        if (record := json.loads(path.read_text()))["unit"][0] != "UNKNOWN"
    }
    if arm == "native":
        test = run(
            ["cargo", "test", "--tests", *common], root, env, directory / "test.log"
        )
        executed = sorted(
            re.findall(
                r"^test (.+?) \.\.\. (ok|FAILED|ignored)",
                (directory / "test.log").read_text(),
                re.MULTILINE,
            )
        )
    else:
        test = run(
            [
                once,
                "--memory-limit",
                "2560MiB",
                "--format",
                "json",
                "test",
                "--jobs",
                "1",
            ],
            root,
            env,
            directory / "test.log",
        )
        query = run(
            [once, "--format", "json", "query", "tests"],
            root,
            env,
            directory / "test-targets.json",
        )
        if query:
            raise RuntimeError("Once test-scope query failed")
        executed = []
        for index, target in enumerate(
            json.loads((directory / "test-targets.json").read_text())
        ):
            path = directory / f"test-results-{index}.json"
            status = run(
                [once, "--format", "json", "query", "test-results", target["id"]],
                root,
                env,
                path,
            )
            if status:
                executed.append((target["id"], "UNMATCHED_RESULTS"))
            else:
                data = json.loads(path.read_text())
                for case in data["cases"]:
                    executed.append(
                        (
                            case["name"],
                            {
                                "passed": "ok",
                                "failed": "FAILED",
                                "skipped": "ignored",
                            }.get(case["status"], case["status"]),
                        )
                    )
        executed.sort()
        if not seed:
            for index in (1, 2):
                status = run(
                    [once, "--format", "json", "query", "graph-fingerprint"],
                    root,
                    env,
                    directory / f"graph-{index}.json",
                )
                if status:
                    raise RuntimeError("graph repeatability observation failed")
    records = [json.loads(path.read_text()) for path in sorted(logs.glob("*.json"))]
    if not records:
        raise RuntimeError(f"no instrumented compiler observations: {directory}")
    units = {}
    tests = {}
    unmatched = []
    for record in records:
        if record["unit"][0] == "UNKNOWN":
            unmatched.append(record["unit"])
            continue
        unit = json.dumps(record["unit"])
        observation = {
            key: record[key]
            for key in (
                "sources",
                "externs",
                "cfg",
                "codegen",
                "edition",
                "target",
                "crate_type",
                "generated",
                "exit",
            )
        }
        observation["marker"] = record["env"].get("E7_MARKER")
        if unit in units and units[unit] != observation:
            raise RuntimeError(f"conflicting repeated observation: {unit}")
        units[unit] = observation
        if "executable" in record and unit not in tests:
            output = directory / ("cases-" + str(len(tests)) + ".log")
            status = run([record["executable"], "--test-threads=1"], root, env, output)
            cases = re.findall(
                r"^test (.+?) \.\.\. (ok|FAILED|ignored)",
                output.read_text(),
                re.MULTILINE,
            )
            tests[unit] = {"exit": status, "cases": sorted(cases)}
    result = {
        "build_exit": build,
        "build_units": sorted(build_units),
        "test_exit": test,
        "units": units,
        "tests": tests,
        "executed_cases": executed,
        "unmatched_support_compilations": unmatched,
        "compiler_invocations": len(records),
        "directory": directory.name,
    }
    if arm == "once" and not seed:
        result["graph_repeatable"] = (directory / "graph-1.json").read_bytes() == (
            directory / "graph-2.json"
        ).read_bytes()
    (directory / "observations.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def compare(native, once):
    differences = []
    for field in ("build_exit", "build_units", "test_exit", "tests", "executed_cases"):
        if native[field] != once[field]:
            differences.append(
                {"field": field, "native": native[field], "once": once[field]}
            )
    for unit in sorted(native["units"].keys() | once["units"].keys()):
        left, right = native["units"].get(unit), once["units"].get(unit)
        if left is None or right is None:
            differences.append(
                {"unit": unit, "field": "unit_scope", "native": left, "once": right}
            )
        else:
            for field in left:
                if left[field] != right[field]:
                    differences.append(
                        {
                            "unit": unit,
                            "field": field,
                            "native": left[field],
                            "once": right[field],
                        }
                    )
    return differences


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--once", type=Path, required=True)
    parser.add_argument(
        "--output", type=Path, required=True, help="new directory for evidence"
    )
    parser.add_argument("--seeds", action="store_true")
    args = parser.parse_args()
    once = str(args.once.resolve(strict=True))
    env = clean_environment()
    real = (
        subprocess.check_output(
            ["rustc", "--print", "sysroot"], env=env, text=True
        ).strip()
        + "/bin/rustc"
    )
    version = subprocess.check_output([real, "-Vv"], env=env, text=True)
    if "rustc 1.96.0 " not in version or f"host: {TRIPLE}\n" not in version:
        parser.error("this bounded experiment requires Rust 1.96.0 on " + TRIPLE)
    if shutil.which("pkill") is None:
        parser.error("this Linux-only experiment requires procps pkill")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "schema": "once.cargo-parity-experiment.v1",
        "toolchain": version,
        "source_revision": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=HERE, text=True
        ).strip(),
        "cargo_version": subprocess.check_output(
            ["cargo", "-V"], env=env, text=True
        ).strip(),
        "once_sha256": file_digest(Path(once)),
        "harness_sha256": file_digest(Path(__file__)),
        "fixture_sha256": {
            str(path.relative_to(HERE)): file_digest(path)
            for path in sorted((HERE / "fixtures").rglob("*"))
            if path.is_file()
        },
        "selection": {
            "target": TRIPLE,
            "features": ["extra"],
            "default_features": False,
            "profile": "dev",
            "opt_level": 0,
            "debug": 2,
            "test_scope": "unit + integration, no doctests",
            "native_jobs": 1,
            "once_test_workers": 1,
            "once_memory_bytes": 2560 * 1024 * 1024,
        },
        "trials": {},
        "seed_detection": {},
        "unsupported": [
            "other platforms/profiles/toolchains",
            "host/target/profile variants sharing a package/root/test identity",
            "doctests, benches, examples, proc macros",
            "registry/git dependencies and native linking",
            "binary byte equality",
            "arbitrary Cargo config and whitespace in dep-info paths",
            "cache replay, sandbox/remote execution, incremental invalidation",
        ],
    }
    with tempfile.TemporaryDirectory(prefix="once-e7-") as temporary:
        base = Path(temporary)
        for fixture in ("tiny", "complex"):
            print("observing " + fixture, flush=True)
            native = observe(base, fixture, "native", None, once, real, env)
            control = observe(base, fixture, "once", None, once, real, env)
            baseline = compare(native, control)
            report["trials"][fixture] = {
                "native": native,
                "once": control,
                "differences": baseline,
                "controls_succeeded": all(
                    trial["build_exit"] == 0
                    and trial["test_exit"] == 0
                    and trial["units"]
                    and trial["tests"]
                    and all(
                        unit["cfg"] != ["UNKNOWN"] for unit in trial["units"].values()
                    )
                    for trial in (native, control)
                ),
            }
            if args.seeds:
                seeds = (
                    ("feature", "source", "test")
                    if fixture == "tiny"
                    else ("build-dependency", "generated-file")
                )
                for seed in seeds:
                    print("seeding " + seed, flush=True)
                    observed = observe(base, fixture, "once", seed, once, real, env)
                    differences = compare(native, observed)
                    added = [diff for diff in differences if diff not in baseline]
                    required = {
                        "feature": "cfg",
                        "source": "sources",
                        "build-dependency": "externs",
                        "generated-file": "generated",
                        "test": "tests",
                    }[seed]
                    detected = any(diff["field"] == required for diff in added)
                    if seed in ("source", "build-dependency", "test"):
                        detected = (
                            detected
                            and observed["build_exit"] == observed["test_exit"] == 0
                        )
                    if seed == "generated-file":
                        diagnostic = (
                            base / observed["directory"] / "build.log"
                        ).read_text()
                        detected = (
                            detected
                            and "couldn't read" in diagnostic
                            and "generated.rs" in diagnostic
                        )
                    report["seed_detection"][seed] = {
                        "detected": detected,
                        "required_field": required,
                        "new_differences": added,
                        "once": observed,
                    }
        # Retain evidence, but remove executables, caches and private toolchain proxies.
        for directory in sorted(base.iterdir()):
            destination = output / directory.name
            destination.mkdir()
            for file in directory.iterdir():
                if file.is_file():
                    shutil.copy2(file, destination / file.name)
            shutil.copytree(directory / "compiler", destination / "compiler")
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    detected = sum(value["detected"] for value in report["seed_detection"].values())
    discrepancies = sum(
        len(trial["differences"]) for trial in report["trials"].values()
    )
    print(
        f"baseline discrepancies: {discrepancies}; seeded mismatches detected: {detected}/{len(report['seed_detection'])}"
    )
    print(output / "report.json")
    gates = all(
        trial["controls_succeeded"] and trial["once"]["graph_repeatable"]
        for trial in report["trials"].values()
    )
    return (
        1
        if not gates or discrepancies or (args.seeds and detected != len(SEEDS))
        else 0
    )


def file_digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "_rustc":
        sys.exit(compiler(*sys.argv[2:6], sys.argv[6:]))
    sys.exit(main())
