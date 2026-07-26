#!/usr/bin/env python3
# once input "../CMakeLists.txt"
# once input "../include"
# once input "../src"
# once output "../cmake-snapshot.json"
# once cwd ".."

import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
STATE = ROOT / ".once" / "tmp" / "cmake-refresh"
BUILD = STATE / "build"
REPLY = BUILD / ".cmake" / "api" / "v1" / "reply"
CONFIGURATION = "Release"
GENERATOR = "Ninja"


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def inside(path, root):
    try:
        return path.relative_to(root)
    except ValueError:
        return None


def normalize_path(value):
    if not isinstance(value, str) or not value:
        return value
    path = Path(value)
    if not path.is_absolute():
        path = ROOT / path
    path = path.resolve(strict=False)
    relative = inside(path, BUILD)
    if relative is not None:
        return "$build/" + relative.as_posix()
    relative = inside(path, ROOT)
    if relative is not None:
        return relative.as_posix() or "."
    return "$host/" + path.name


def normalize_value(value):
    if isinstance(value, str):
        root = str(ROOT)
        build = str(BUILD)
        return value.replace(build, "$build").replace(root, "$source")
    if isinstance(value, list):
        return [normalize_value(item) for item in value]
    if isinstance(value, dict):
        return {key: normalize_value(item) for key, item in value.items()}
    return value


def find_reply(index, kind):
    for value in index.get("reply", {}).values():
        if isinstance(value, dict) and value.get("kind") == kind:
            return read_json(REPLY / value["jsonFile"])
    return {}


def target_name(name, target_id, used):
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "-", name).strip("-") or "target"
    candidate = "cmake-" + slug
    if candidate in used:
        suffix = hashlib.sha256(target_id.encode("utf-8")).hexdigest()[:8]
        candidate += "-" + suffix
    used.add(candidate)
    return candidate


def configuration_inputs(cmake_files):
    inputs = {}
    paths = []
    for entry in cmake_files.get("inputs", []):
        if entry.get("isGenerated"):
            continue
        value = entry.get("path")
        if not value:
            continue
        path = Path(value)
        if not path.is_absolute():
            path = ROOT / path
        path = path.resolve(strict=False)
        relative = inside(path, ROOT)
        if relative is None or not path.is_file():
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        key = relative.as_posix()
        inputs[key] = content
        paths.append(key)
    cmake_lists = ROOT / "CMakeLists.txt"
    inputs.setdefault("CMakeLists.txt", cmake_lists.read_text(encoding="utf-8"))
    paths.append("CMakeLists.txt")
    return dict(sorted(inputs.items())), sorted(set(paths))


def normalized_toolchains(toolchains):
    result = []
    for toolchain in toolchains.get("toolchains", []):
        compiler = toolchain.get("compiler", {})
        result.append(
            {
                "language": toolchain.get("language", ""),
                "compiler": {
                    "id": compiler.get("id", ""),
                    "version": compiler.get("version", ""),
                    "path": Path(compiler.get("path", "")).name,
                    "target": compiler.get("target", ""),
                },
            }
        )
    return sorted(result, key=lambda item: item["language"])


def normalized_cache(cache):
    result = []
    for entry in cache.get("entries", []):
        name = entry.get("name", "")
        entry_type = entry.get("type", "")
        include = (
            name == "CMAKE_BUILD_TYPE"
            or name == "CMAKE_INSTALL_PREFIX"
            or name.startswith("CMAKE_C_FLAGS")
            or (name and not name.startswith("CMAKE_"))
        )
        if not include or entry_type == "INTERNAL":
            continue
        value = entry.get("value")
        if entry_type in {"FILEPATH", "PATH"} and isinstance(value, str):
            value = normalize_path(value)
        else:
            value = normalize_value(value)
        result.append(
            {
                "name": name,
                "type": entry_type,
                "value": value,
            }
        )
    return sorted(result, key=lambda item: item["name"])


def target_records(codemodel):
    configurations = codemodel.get("configurations", [])
    configuration = next(
        (item for item in configurations if item.get("name") == CONFIGURATION),
        configurations[0] if configurations else {},
    )
    references = configuration.get("targets", [])
    details = [read_json(REPLY / reference["jsonFile"]) for reference in references]
    used = set()
    names = {
        reference["id"]: target_name(reference.get("name", ""), reference["id"], used)
        for reference in references
    }
    records = []
    for reference, detail in zip(references, details):
        include_dirs = []
        definitions = []
        for group in detail.get("compileGroups", []):
            include_dirs.extend(
                normalize_path(item.get("path", ""))
                for item in group.get("includes", [])
                if item.get("path")
            )
            definitions.extend(
                item.get("define", "")
                for item in group.get("defines", [])
                if item.get("define")
            )
        sources = [
            normalize_path(item["path"])
            for item in detail.get("sources", [])
            if item.get("path")
        ]
        artifacts = [
            normalize_path(item["path"])
            for item in detail.get("artifacts", [])
            if item.get("path")
        ]
        dependencies = [
            names[item["id"]]
            for item in detail.get("dependencies", [])
            if item.get("id") in names
        ]
        records.append(
            {
                "name": detail.get("name", reference.get("name", "")),
                "once_name": names[reference["id"]],
                "type": detail.get("type", ""),
                "deps": sorted(set(dependencies)),
                "sources": sorted(set(sources)),
                "artifacts": sorted(set(artifacts)),
                "include_dirs": sorted(set(include_dirs)),
                "compile_definitions": sorted(set(definitions)),
            }
        )
    records.sort(key=lambda item: item["once_name"])
    return records


def test_records(ctest):
    records = []
    for test in ctest.get("tests", []):
        command = test.get("command", [])
        if command:
            command = [normalize_path(command[0])] + [
                normalize_value(value) for value in command[1:]
            ]
        records.append(
            {
                "name": test.get("name", ""),
                "command": command,
                "properties": normalize_value(test.get("properties", [])),
            }
        )
    return sorted(records, key=lambda item: item["name"])


def cmake_version(cmake):
    output = subprocess.run(
        [cmake, "--version"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout
    return output.splitlines()[0].removeprefix("cmake version ").strip()


def configure(cmake):
    if STATE.exists():
        shutil.rmtree(STATE)
    query = BUILD / ".cmake" / "api" / "v1" / "query"
    query.mkdir(parents=True)
    for name in ["codemodel-v2", "cache-v2", "cmakeFiles-v1", "toolchains-v1"]:
        (query / name).write_text("", encoding="utf-8")
    subprocess.run(
        [
            cmake,
            "-S",
            str(ROOT),
            "-B",
            str(BUILD),
            "-G",
            GENERATOR,
            "-DCMAKE_BUILD_TYPE=" + CONFIGURATION,
        ],
        check=True,
    )


def ctest_records(cmake):
    ctest = str(Path(cmake).with_name("ctest"))
    output = subprocess.run(
        [
            ctest,
            "--show-only=json-v1",
            "--test-dir",
            str(BUILD),
            "-C",
            CONFIGURATION,
        ],
        check=True,
        text=True,
        capture_output=True,
    ).stdout
    return read_json_text(output)


def read_json_text(value):
    return json.loads(value)


def main():
    cmake = shutil.which("cmake")
    if not cmake:
        raise SystemExit("cmake was not found on PATH")
    configure(cmake)
    index_path = sorted(REPLY.glob("index-*.json"))[-1]
    index = read_json(index_path)
    codemodel = find_reply(index, "codemodel")
    cache = find_reply(index, "cache")
    cmake_files = find_reply(index, "cmakeFiles")
    toolchains = find_reply(index, "toolchains")
    inputs, input_paths = configuration_inputs(cmake_files)
    targets = target_records(codemodel)
    exports = [
        target["name"]
        for target in targets
        if target["artifacts"]
        and target["type"] not in {"UTILITY", "INTERFACE_LIBRARY"}
    ]
    snapshot = {
        "schema": "once.cmake.snapshot.v1",
        "once_snapshot": {
            "inputs": inputs,
            "selection": {
                "source_dir": ".",
                "generator": GENERATOR,
                "build_type": CONFIGURATION,
            },
        },
        "cmake": {
            "version": cmake_version(cmake),
            "generator": index.get("cmake", {}).get("generator", {}).get("name", GENERATOR),
            "configuration": CONFIGURATION,
            "configuration_inputs": input_paths,
            "cache": normalized_cache(cache),
            "toolchains": normalized_toolchains(toolchains),
        },
        "targets": targets,
        "exports": exports,
        "tests": test_records(ctest_records(cmake)),
    }
    fingerprint = hashlib.sha256(canonical(snapshot).encode("utf-8")).hexdigest()
    snapshot["once_snapshot"]["fingerprint"] = fingerprint
    for target in snapshot["targets"]:
        target["snapshot_fingerprint"] = fingerprint
    destination = ROOT / "cmake-snapshot.json"
    destination.write_text(
        json.dumps(snapshot, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(destination)


if __name__ == "__main__":
    main()
