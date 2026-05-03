"""Built-in Rust target types.

This file is bundled into the fabrik binary and evaluated implicitly
in every user fabrik.star file's namespace before user code runs. It
defines the Rust target types in terms of the lower-level `target(...)`
primitive, so the same plugin contract third-party plugins will use
already drives the built-in ones.
"""

def rust_binary(name, srcs = [], deps = []):
    """A Rust binary crate (will compile with --crate-type=bin)."""
    target(kind = "rust_binary", name = name, srcs = srcs, deps = deps)

def rust_library(name, srcs = [], deps = []):
    """A Rust library crate (will compile with --crate-type=lib)."""
    target(kind = "rust_library", name = name, srcs = srcs, deps = deps)

def rust_test(name, srcs = [], deps = []):
    """A Rust test target (will compile with --test)."""
    target(kind = "rust_test", name = name, srcs = srcs, deps = deps)

def cargo_binary(name, cargo_package = "", bin = "", srcs = [], deps = []):
    """A Cargo-built binary target for adopted Rust workspaces."""
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
