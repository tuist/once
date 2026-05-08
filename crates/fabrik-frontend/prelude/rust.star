"""Built-in Rust target types.

This file is bundled into the fabrik binary and evaluated implicitly
in every user fabrik.star file's namespace before user code runs. It
defines the Rust target types in terms of the lower-level `target(...)`
primitive, so the same plugin contract third-party plugins will use
already drives the built-in ones.
"""

def _rust_attrs(crate_name, edition, crate_root):
    """Pack the optional rustc-shaped settings into the typed attrs
    dict the target primitive expects. Empty strings are dropped so
    the action digest doesn't churn on default values.
    """
    attrs = {}
    if crate_name != "":
        attrs["crate_name"] = crate_name
    if edition != "":
        attrs["edition"] = edition
    if crate_root != "":
        attrs["crate_root"] = crate_root
    return attrs

def rust_binary(
    name,
    srcs = [],
    deps = [],
    crate_name = "",
    edition = "",
    crate_root = ""):
    """A Rust binary crate (compiles with --crate-type=bin)."""
    target(
        kind = "rust_binary",
        name = name,
        srcs = srcs,
        deps = deps,
        attrs = _rust_attrs(crate_name, edition, crate_root),
    )

def rust_library(
    name,
    srcs = [],
    deps = [],
    crate_name = "",
    edition = "",
    crate_root = ""):
    """A Rust library crate (compiles with --crate-type=lib, emits
    rlib + rmeta so dependents type-check against the metadata).
    """
    target(
        kind = "rust_library",
        name = name,
        srcs = srcs,
        deps = deps,
        attrs = _rust_attrs(crate_name, edition, crate_root),
    )

def rust_test(
    name,
    srcs = [],
    deps = [],
    crate_name = "",
    edition = "",
    crate_root = ""):
    """A Rust test target (compiles with --test)."""
    target(
        kind = "rust_test",
        name = name,
        srcs = srcs,
        deps = deps,
        attrs = _rust_attrs(crate_name, edition, crate_root),
    )

def rust_proc_macro(
    name,
    srcs = [],
    deps = [],
    crate_name = "",
    edition = "",
    crate_root = ""):
    """A procedural-macro crate (compiles with --crate-type=proc-macro,
    cached under a separate namespace because it builds for the host
    platform regardless of the target triple).
    """
    target(
        kind = "rust_proc_macro",
        name = name,
        srcs = srcs,
        deps = deps,
        attrs = _rust_attrs(crate_name, edition, crate_root),
    )

def cargo_crate(
    name,
    srcs = [],
    deps = [],
    crate_name = "",
    edition = "2021",
    crate_root = "src/lib.rs",
    proc_macro = False):
    """A vendored crates.io dependency, compiled the same granular way
    as a hand-written `rust_library` (or `rust_proc_macro`).

    Intended to be emitted by `fabrik vendor`, not hand-written. The
    generator walks `cargo metadata` and writes one of these per
    resolved dep, pointing at sources copied under `vendor/<crate>-<version>/`.

    Crates that need a `build.rs` or that depend on a build script's
    outputs are not yet handled by the granular pipeline; the
    generator emits a comment for them and the user is expected to
    fall back to `cargo_binary` for the affected subgraph.
    """
    if proc_macro:
        rust_proc_macro(
            name = name,
            srcs = srcs,
            deps = deps,
            crate_name = crate_name,
            edition = edition,
            crate_root = crate_root,
        )
    else:
        rust_library(
            name = name,
            srcs = srcs,
            deps = deps,
            crate_name = crate_name,
            edition = edition,
            crate_root = crate_root,
        )

def cargo_build_script(name, srcs = [], deps = [], crate_dir = ""):
    """A `build.rs`-style build script.

    Compiles `srcs` (which must include a `build.rs` file), runs the
    resulting binary under cargo's standard build-script env, and
    captures stdout to a deterministic file under `.fabrik/out/...`.
    Downstream consumption (auto-threading captured directives into
    dependent rustc invocations) is not yet implemented; for now
    consumers parse the captured artifact themselves or fall back to
    `cargo_binary`.
    """
    attrs = {}
    if crate_dir != "":
        attrs["crate_dir"] = crate_dir
    target(
        kind = "cargo_build_script",
        name = name,
        srcs = srcs,
        deps = deps,
        attrs = attrs,
    )

def cargo_binary(name, cargo_package = "", bin = "", srcs = [], deps = []):
    """A Cargo-built binary target for adopted Rust workspaces.

    Provided as an escape hatch for projects whose third-party graph
    is not yet expressible as granular `rust_*` targets. Falls through
    to `cargo build`, so cache granularity matches Cargo's, not Fabrik's.
    """
    if cargo_package == "":
        cargo_package = name
    if bin == "":
        bin = name
    target(
        kind = "cargo_binary",
        name = name,
        srcs = srcs,
        deps = deps,
        attrs = {
            "cargo_package": cargo_package,
            "bin": bin,
        },
    )
