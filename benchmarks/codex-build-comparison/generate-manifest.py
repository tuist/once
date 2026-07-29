#!/usr/bin/env python3

import argparse
import json
import os
import subprocess
from collections import Counter
from pathlib import Path


def quoted(value):
    return json.dumps(value, ensure_ascii=False)


def array(values):
    return "[" + ", ".join(quoted(value) for value in values) + "]"


def string_map(values):
    entries = [
        f"{quoted(key)} = {quoted(value)}"
        for key, value in sorted(values.items())
        if value is not None
    ]
    return "{ " + ", ".join(entries) + " }"


def dependency_kinds(dependency):
    return {
        item.get("kind")
        for item in dependency.get("dep_kinds") or [{"kind": None}]
    }


def library_target(package):
    for target in package.get("targets", []):
        kinds = target.get("kind", [])
        crate_types = target.get("crate_types", [])
        if "proc-macro" in kinds or "proc-macro" in crate_types:
            return target
    for target in package.get("targets", []):
        kinds = target.get("kind", [])
        crate_types = target.get("crate_types", [])
        if "lib" in kinds or "lib" in crate_types or "rlib" in crate_types:
            return target
    return None


def build_script_target(package):
    return next(
        (
            target
            for target in package.get("targets", [])
            if "custom-build" in target.get("kind", [])
        ),
        None,
    )


def package_environment(package, package_path):
    version = package.get("version", "0.0.0")
    core, _, pre = version.partition("-")
    parts = core.split(".")
    while len(parts) < 3:
        parts.append("0")
    license_file = package.get("license_file")
    readme = package.get("readme")
    return {
        "BAZEL_PACKAGE": package_path,
        "CARGO_MANIFEST_DIR": package_path,
        "CARGO_MANIFEST_PATH": f"{package_path}/Cargo.toml",
        "CARGO_PKG_AUTHORS": ":".join(package.get("authors") or []),
        "CARGO_PKG_DESCRIPTION": package.get("description") or "",
        "CARGO_PKG_HOMEPAGE": package.get("homepage") or "",
        "CARGO_PKG_LICENSE": package.get("license") or "",
        "CARGO_PKG_LICENSE_FILE": license_file or "",
        "CARGO_PKG_NAME": package["name"],
        "CARGO_PKG_README": readme or "",
        "CARGO_PKG_REPOSITORY": package.get("repository") or "",
        "CARGO_PKG_RUST_VERSION": package.get("rust_version") or "",
        "CARGO_PKG_VERSION": version,
        "CARGO_PKG_VERSION_MAJOR": parts[0],
        "CARGO_PKG_VERSION_MINOR": parts[1],
        "CARGO_PKG_VERSION_PATCH": parts[2],
        "CARGO_PKG_VERSION_PRE": pre,
    }


def external_target_names(packages):
    external = [package for package in packages if package.get("source")]
    duplicate_counts = Counter(
        (package["name"], package["version"]) for package in external
    )
    names = {}
    for package in external:
        name = f"{package['name']}-{package['version']}"
        if duplicate_counts[(package["name"], package["version"])] > 1:
            suffix = "".join(
                character
                if character.isascii()
                and (character.isalnum() or character in "_.-")
                else "_"
                for character in package.get("source", "")
            )
            name += f"-{suffix}"
        names[package["id"]] = name
    return names


def normal_dependencies(node):
    return [
        dependency
        for dependency in node.get("deps", [])
        if None in dependency_kinds(dependency)
    ]


def build_dependencies(node):
    return [
        dependency
        for dependency in node.get("deps", [])
        if "build" in dependency_kinds(dependency)
    ]


def workspace_closure(root_id, nodes, workspace_ids):
    closure = set()
    pending = [root_id]
    while pending:
        package_id = pending.pop()
        if package_id in closure:
            continue
        closure.add(package_id)
        node = nodes[package_id]
        for dependency in normal_dependencies(node) + build_dependencies(node):
            if dependency["pkg"] in workspace_ids:
                pending.append(dependency["pkg"])
    return closure


def selected_features(manifest, target):
    command = [
        "cargo",
        "tree",
        "--locked",
        "--offline",
        "--manifest-path",
        str(manifest),
        "--target",
        target,
        "-p",
        "codex-cli",
        "-e",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}|{f}",
    ]
    environment = dict(os.environ)
    environment["CARGO_TERM_COLOR"] = "never"
    output = subprocess.run(
        command,
        check=True,
        capture_output=True,
        env=environment,
        text=True,
    ).stdout
    features = {}
    for line in output.splitlines():
        package, separator, feature_text = line.partition("|")
        if not separator:
            continue
        fields = package.split()
        if len(fields) < 2 or not fields[1].startswith("v"):
            continue
        key = (fields[0], fields[1][1:])
        package_features = features.setdefault(key, set())
        for feature in feature_text.split(","):
            feature = feature.strip()
            if feature.endswith(" (*)"):
                feature = feature.removesuffix(" (*)")
            if feature and feature != "(*)":
                package_features.add(feature)
    return features


def pruned_metadata(metadata, root_id, manifest, target):
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    features = selected_features(manifest, target)
    closure = {
        package["id"]
        for package in metadata["packages"]
        if (package["name"], package["version"]) in features
    }
    closure.add(root_id)
    for package in metadata["packages"]:
        if package["id"] not in closure:
            continue
        nodes[package["id"]]["features"] = sorted(
            features.get((package["name"], package["version"]), set())
        )
        kept_dependencies = []
        for dependency in nodes[package["id"]].get("deps", []):
            if dependency["pkg"] not in closure:
                continue
            dep_kinds = [
                item
                for item in dependency.get("dep_kinds", [])
                if item.get("kind") in (None, "build")
            ]
            if not dep_kinds:
                continue
            kept_dependency = dict(dependency)
            kept_dependency["dep_kinds"] = dep_kinds
            kept_dependencies.append(kept_dependency)
        nodes[package["id"]]["deps"] = kept_dependencies

    result = dict(metadata)
    result["packages"] = [
        package for package in metadata["packages"] if package["id"] in closure
    ]
    result["workspace_members"] = [
        package_id
        for package_id in metadata["workspace_members"]
        if package_id in closure
    ]
    result["workspace_default_members"] = [root_id]
    result["resolve"] = dict(metadata["resolve"])
    result["resolve"]["nodes"] = [
        nodes[node["id"]]
        for node in metadata["resolve"]["nodes"]
        if node["id"] in closure
    ]
    result["resolve"]["root"] = root_id
    return result


def snapshot_inputs(root):
    paths = {root / "codex-rs" / "Cargo.toml", root / "codex-rs" / "Cargo.lock"}
    paths.update((root / "codex-rs").rglob("Cargo.toml"))
    return {
        path.relative_to(root).as_posix(): path.read_text()
        for path in sorted(paths)
    }


def relative_path(path, root):
    return Path(path).resolve().relative_to(root).as_posix()


def append_target(
    lines,
    package,
    target,
    node,
    root,
    workspace_ids,
    target_names,
    library_targets,
    external_names,
):
    package_path = relative_path(Path(package["manifest_path"]).parent, root)
    crate_root = relative_path(target["src_path"], root)
    name = target_names[package["id"]]
    kind = (
        "rust_proc_macro"
        if "proc-macro" in target.get("kind", [])
        or "proc-macro" in target.get("crate_types", [])
        else "rust_library"
    )
    normal = normal_dependencies(node)
    dependencies = [
        target_names[dependency["pkg"]]
        for dependency in normal
        if dependency["pkg"] in workspace_ids
        and dependency["pkg"] in library_targets
    ]
    dependencies.append("cargo_dependencies")
    build = build_dependencies(node)
    build_dependency_names = [
        target_names[dependency["pkg"]]
        for dependency in build
        if dependency["pkg"] in workspace_ids
        and dependency["pkg"] in library_targets
    ]
    build_dependency_names.extend(
        f"{external_names[dependency['pkg']]}-host"
        for dependency in build
        if dependency["pkg"] in external_names
    )
    aliases = {}
    for dependency in normal + build:
        dependency_target = library_targets.get(dependency["pkg"])
        if dependency_target is None:
            continue
        expected_name = dependency["name"]
        actual_name = dependency_target["name"]
        if expected_name != actual_name:
            aliases[target_names[dependency["pkg"]]] = expected_name

    lines.extend(
        [
            "",
            "[[target]]",
            f"name = {quoted(name)}",
            f"kind = {quoted(kind)}",
            f"deps = {array(sorted(set(dependencies)))}",
            f"srcs = {array([package_path + '/**/*'])}",
            "",
            "[target.attrs]",
            f"crate_name = {quoted(target['name'])}",
            f"crate_root = {quoted(crate_root)}",
            f"edition = {quoted(target.get('edition') or '2021')}",
            f"features = {array(sorted(node.get('features') or []))}",
            f"cargo_package = {quoted(package['name'])}",
            f"env = {string_map(package_environment(package, package_path))}",
        ]
    )
    if aliases:
        lines.append(f"crate_aliases = {string_map(aliases)}")
    build_script = build_script_target(package)
    if build_script is not None:
        lines.append(
            f"build_script = {quoted(relative_path(build_script['src_path'], root))}"
        )
    if build_dependency_names:
        lines.extend(
            [
                "",
                "[target.dependencies]",
                f"build_deps = {array(sorted(set(build_dependency_names)))}",
            ]
        )


def append_binary(
    lines,
    package,
    target,
    node,
    root,
    workspace_ids,
    target_names,
    library_targets,
):
    package_path = relative_path(Path(package["manifest_path"]).parent, root)
    crate_root = relative_path(target["src_path"], root)
    dependencies = [
        target_names[dependency["pkg"]]
        for dependency in normal_dependencies(node)
        if dependency["pkg"] in workspace_ids
        and dependency["pkg"] in library_targets
    ]
    package_library = library_targets.get(package["id"])
    if package_library is not None:
        dependencies.append(target_names[package["id"]])
    dependencies.append("cargo_dependencies")

    lines.extend(
        [
            "",
            "[[target]]",
            f"name = {quoted(target['name'])}",
            'kind = "rust_binary"',
            f"deps = {array(sorted(set(dependencies)))}",
            f"srcs = {array([package_path + '/**/*'])}",
            "",
            "[target.attrs]",
            f"crate_name = {quoted(target['name'].replace('-', '_'))}",
            f"crate_root = {quoted(crate_root)}",
            f"edition = {quoted(target.get('edition') or '2021')}",
            f"features = {array(sorted(node.get('features') or []))}",
            f"cargo_package = {quoted(package['name'])}",
            f"env = {string_map(package_environment(package, package_path))}",
            'linker_flags = ["-ObjC", "-lc++"]',
        ]
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("checkout", type=Path)
    parser.add_argument("--target", default="aarch64-apple-darwin")
    args = parser.parse_args()

    root = args.checkout.resolve()
    manifest = root / "codex-rs" / "Cargo.toml"
    command = [
        "cargo",
        "metadata",
        "--locked",
        "--offline",
        "--format-version",
        "1",
        "--manifest-path",
        str(manifest),
        "--filter-platform",
        args.target,
    ]
    metadata = json.loads(subprocess.run(command, check=True, capture_output=True).stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace_ids = set(metadata["workspace_members"])
    root_package = next(
        package
        for package in packages.values()
        if package["name"] == "codex-cli"
    )
    metadata = pruned_metadata(
        metadata,
        root_package["id"],
        manifest,
        args.target,
    )
    metadata["once_snapshot"] = {
        "inputs": snapshot_inputs(root),
        "selection": {
            "features": [],
            "all_features": False,
            "no_default_features": False,
            "target": "",
            "host": False,
            "host_triple": args.target,
        },
    }
    snapshot_path = root / "once-cargo-metadata.json"
    snapshot_path.write_text(
        json.dumps(metadata, ensure_ascii=False, separators=(",", ":"))
    )
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace_ids = set(metadata["workspace_members"])
    closure = workspace_closure(root_package["id"], nodes, workspace_ids)
    library_targets = {
        package_id: library_target(packages[package_id])
        for package_id in closure
    }
    library_targets = {
        package_id: target
        for package_id, target in library_targets.items()
        if target is not None
    }
    target_names = {
        package_id: packages[package_id]["name"] for package_id in closure
    }
    external_names = external_target_names(packages.values())

    lines = [
        "# Generated by benchmarks/codex-build-comparison/generate-manifest.py.",
        "",
        "[workspace]",
        'include = ["once.toml"]',
        "",
        "[[target]]",
        'name = "cargo_dependencies"',
        'kind = "cargo_dependencies"',
        'srcs = ["codex-rs/Cargo.toml", "codex-rs/Cargo.lock", "codex-rs/**/Cargo.toml", "once-cargo-metadata.json"]',
        "",
        "[target.attrs]",
        'manifest = "codex-rs/Cargo.toml"',
        'lockfile = "codex-rs/Cargo.lock"',
        'resolver_inputs = ["codex-rs/Cargo.toml", "codex-rs/Cargo.lock", "codex-rs/**/Cargo.toml", "once-cargo-metadata.json"]',
        'metadata_file = "once-cargo-metadata.json"',
        'vendor_dir = "third_party/rust/vendor"',
    ]

    for package_id in sorted(
        library_targets, key=lambda item: target_names[item]
    ):
        append_target(
            lines,
            packages[package_id],
            library_targets[package_id],
            nodes[package_id],
            root,
            workspace_ids,
            target_names,
            library_targets,
            external_names,
        )

    binary = next(
        target
        for target in root_package["targets"]
        if target["name"] == "codex" and "bin" in target.get("kind", [])
    )
    append_binary(
        lines,
        root_package,
        binary,
        nodes[root_package["id"]],
        root,
        workspace_ids,
        target_names,
        library_targets,
    )

    (root / "once.toml").write_text("\n".join(lines) + "\n")
    print(
        json.dumps(
            {
                "manifest": str(root / "once.toml"),
                "workspace_packages": len(closure),
                "first_party_targets": len(library_targets) + 1,
                "resolved_packages": len(packages),
                "snapshot": str(snapshot_path),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
