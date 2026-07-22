---
prev: false
next: false
---

# Zig

Once can describe Zig modules, build and run executables, run host tests, and
produce static or shared libraries. This guide uses one math module from a
binary and a test target.

## Prerequisites

Install the repository's pinned Zig compiler through mise:

```sh
mise install
mise exec -- zig version
```

Zig targets resolve `zig` from the command search path by default. Set the
`zig` attribute for a different executable. Set `zig_version` when analysis
should reject a compiler whose reported version does not match the project.

## Declare a Module, Binary, and Test

Create `tools/hello/once.toml`:

```toml
[[target]]
name = "math"
kind = "zig_library"
srcs = ["src/**/*.zig"]

[target.attrs]
main = "src/math.zig"
import_name = "math"

[[target]]
name = "hello"
kind = "zig_binary"
srcs = ["src/**/*.zig"]
deps = ["./math"]

[target.attrs]
main = "src/main.zig"

[[target]]
name = "math_tests"
kind = "zig_test"
srcs = ["src/**/*.zig"]
deps = ["./math"]

[target.attrs]
main = "src/math_test.zig"
labels = ["unit"]
```

Use this source layout:

```text
tools/hello/
├── once.toml
└── src/
    ├── main.zig
    ├── math.zig
    └── math_test.zig
```

The binary and test import the module with `@import("math")`. The
`import_name` and `deps` entry make that name explicit.

## Import Locked Zig Packages

Use [`zig_dependencies`](/reference/prelude/zig_dependencies) when the project
already declares packages in `build.zig.zon`. Zig's package manifest remains
the source of truth for dependency aliases, local paths, lazy packages, source
locations, and content multihashes.

```toml
[[target]]
name = "zig_dependencies"
kind = "zig_dependencies"
srcs = [
  "build.zig.zon",
  "third_party/zig/**/build.zig.zon",
]

[target.attrs]
manifest = "build.zig.zon"
resolver_inputs = ["build.zig.zon", "third_party/zig/**/build.zig.zon"]
vendor_dir = "third_party/zig"

[[target]]
name = "hello"
kind = "zig_binary"
srcs = ["src/**/*.zig"]
deps = ["zig_dependencies"]

[target.attrs]
main = "src/main.zig"
```

Once reads the locked package graph and emits one queryable `zig_package`
target per package instance. Local path dependencies are resolved relative to
the manifest that declares them, and the normalized materialized source root is
part of their identity. The same relative spelling in two parent packages
therefore cannot collapse distinct dependencies. Remote packages are expected
under the vendor directory, keyed by their Zig content multihash when present.
Verify those directories through Zig's native fetch workflow before committing
them. Ordinary builds do not fetch or update packages, and their action keys
include the actual source contents. Resolution fails explicitly when a graph
exceeds 10,000 package instances.

The conventional public module path is `src/root.zig`. Use `module_paths` when
a package exposes another module:

```toml
[target.attrs.module_paths]
special_package = "third_party/zig/special/source/main.zig"
```

Use `package_paths` to map an alias or content multihash to a different
materialized source directory. This is useful when an existing vendoring
layout cannot be changed. Run Zig's native package fetch workflow explicitly
when the lock or vendored source set changes, then review the expanded graph
with `once query targets --kind zig_package`.

Build and run a first-party consumer after inspecting the imported packages:

```sh
once query targets --kind zig_package
once build hello
once run hello
```

The bundled dependency starter imports a locked `math` package and prints its
answer, `42`, from the local binary. Compilation fails if the dependency set
does not propagate both the edge-local import alias and the package module, so
this run verifies the complete provider handoff.

## Query Before Building

Inspect the targets and their capabilities:

```sh
once query targets --kind zig_library
once query capabilities tools/hello/math
once query capabilities tools/hello/hello
once query capabilities tools/hello/math_tests
once query schema zig_binary
once query schema zig_test
```

The module exposes `build` but does not compile an artifact by itself. Its
binary and test consumers compile it as part of their whole program. The
binary exposes `build` and `run`; the test exposes `build` and `test`.

## Build and Run

Build the executable and its module:

```sh
once build tools/hello/hello
```

The executable appears at `.once/out/tools/hello/hello/hello`, with `.exe`
added on Windows. Zig documentation appears at
`.once/out/tools/hello/hello/hello.docs`. Optional assembly and compiler
representation outputs are produced only when their corresponding attributes
are enabled.

Run the same binary:

```sh
once run tools/hello/hello
```

The binary can receive `args`, `env`, and declared `data` inputs. Run output is
stored under `.once/out/tools/hello/hello/run/`, including `stdout.log` and
`run.json`. Running is not replayed from the action cache, so each invocation
executes the program again.

## Run the Test

Build and execute the host test target:

```sh
once test tools/hello/math_tests
```

The compiled test binary appears under
`.once/out/tools/hello/math_tests/math_tests`. Results and logs appear under
`.once/out/tools/hello/math_tests/test/`, including `test_results.json`,
`zig-test.log`, and `native_results.txt`.

Zig test execution is host-only. A cross-target test binary can be built, but
running it requires a platform runner outside `zig_test`.

## Build Libraries and Choose Configuration

Use [`zig_static_library`](/reference/prelude/zig_static_library) or
[`zig_shared_library`](/reference/prelude/zig_shared_library) when another
native target needs a linkable artifact. Both can consume the `math` module
through `deps` and can emit documentation with the library.

Configured variants keep build choices on the target:

- [`zig_configure`](/reference/prelude/zig_configure) builds a static or shared
  library with attributes such as `mode`, `threaded`, and `zig_version`.
- [`zig_configure_binary`](/reference/prelude/zig_configure_binary) provides
  the same configuration shape for an executable.
- [`zig_configure_test`](/reference/prelude/zig_configure_test) provides it for
  a test target.

Use the base kinds until the project needs one of those explicit settings.

## Connect C and C++ Libraries

Zig binary, test, module, static-library, and shared-library targets accept a
[`c_library`](/reference/prelude/c_library) dependency. They receive its
headers, include directories, definitions, archives, dynamic libraries, and
linker options.

[`zig_c_library`](/reference/prelude/zig_c_library) exposes C headers as an
importable Zig module. It requires at least one C header from its dependencies.
Add this bridge only when Zig source needs translated C declarations, rather
than only linking a native archive.

## Target References and Limitations

Start with the references used by this guide:

- [`zig_dependencies`](/reference/prelude/zig_dependencies)
- [`zig_package`](/reference/prelude/zig_package)
- [`zig_library`](/reference/prelude/zig_library)
- [`zig_binary`](/reference/prelude/zig_binary)
- [`zig_test`](/reference/prelude/zig_test)

The [target kind index](/reference/prelude/) links the static, shared,
configured, and C-bridge kinds. Module import renames must identify exactly one
dependency; unknown or ambiguous names fail validation. Cross-target tests can
be built but cannot run through the host-only test capability.

## Next

Add a [`c_library`](/reference/prelude/c_library) dependency when the example
needs native headers or an archive, then query the binary again before
building. Once the module graph builds, runs, and tests, continue with
[Memory](/guide/memory/) to inspect the durable context recorded for those
actions.
