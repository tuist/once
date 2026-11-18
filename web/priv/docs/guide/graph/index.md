# Graph

The Struo graph describes the named parts of a workspace and what Struo can do
with them. Start with one target, inspect it, and build it. Add dependencies or
more specialized target kinds only when the project needs them.

## Try Your Existing Project First

If the repository already uses Cargo, Swift Package Manager, Xcode, or Bazel,
start in its root without creating a file:

```sh
struo query targets
struo build
struo test
struo lint
```

The first command shows what Struo recognized. The remaining three act on the
workspace's own targets by default: `struo build` builds every workspace-owned
target that exposes `build` (failing fast on the first bad build), `struo test`
runs first-party tests, and `struo lint` lints every workspace-owned target
that exposes `lint` (running each one to completion so you see every finding).

Workspace-owned means the target is one of the pieces the repository's own
manifest declares, not one pulled in through a resolved dependency. A test
run on a Cargo project builds and tests the workspace's crates without
running the test suites of the crates they depend on; the same rule applies
to `struo build` and `struo lint`.

The current definition of workspace-owned is heuristic: Struo traces every
target whose dependency chain reaches one of the resolver's declared primary
products. Sibling targets that live in the same manifest but do not depend
on one of those products (a standalone macro plugin, for example) can slip
past the default. Pass the target id explicitly, or use `--all`, to pick
those up. A future release will replace the heuristic with an explicit
origin marker on each target so the default catches every workspace-owned
target without exception.

Pass `--all` (available on all three) to reach past the workspace boundary
and act on every capable target in the loaded graph, resolved dependencies
included. Use it with care on `struo lint --all`: it will run every
lint-capable target in the graph, including vendored third-party code, and
the invocation fails when any finding meets `--fail-on`. `struo lint --all
--fail-on error` is a reasonable starting point when you want to sweep the
whole graph.

Continue with the guide for [Rust](/guide/graph/rust), [Swift
Packages](/guide/graph/swift-packages), [Xcode
Projects](/guide/graph/apple/xcode), or [Bazel](/guide/graph/bazel). Declare
targets only when the automatically derived graph needs a project-specific
boundary or capability.

## Start With One Target

Targets live in package-level `struo.toml` files. This example declares an
Apple library in `apps/ios/struo.toml`:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
```

The manifest location and target name form the target identifier
`apps/ios/AppCore`. Query it before running any work:

```sh
struo query targets
struo query capabilities apps/ios/AppCore
struo query schema apple_library
```

The first command lists the workspace. The second shows what `AppCore` can do.
The third explains which attributes, dependencies, outputs, and capabilities
an `apple_library` accepts.

Build the same target:

```sh
struo build apps/ios/AppCore
```

Outputs are materialized under `.struo/out/<target>/`. The
[target kind reference](/reference/prelude/) lists the exact output groups for
each kind.

## Compare Two Fresh Builds

Use `--verify-reproducible` when you want to compare repeated execution instead
of checking whether a normal build can reuse its cached result:

```sh
struo build --verify-reproducible apps/service/Service
```

Struo snapshots the workspace and builds the target, including its dependency
closure, in each of two private trials. Each trial starts from that same
snapshot at the same private path with a separate, initially empty local Struo
cache. Struo retains both results and their declared artifacts rather than
replacing outputs in your original workspace.

Both trials must succeed. Declared artifact differences fail the check;
differences only in captured logs are advisory when artifacts match. If no
artifacts are declared, captured logs determine the comparison instead. The
result prints a verification UUID and a command to inspect the retained trials:

```sh
struo query verification <ID>
struo query verification <ID> --format json
```

A passing check means agreement under that artifact/log policy, not proof of
reproducibility. These are not hermetic builds: host toolchains and tool caches,
absolute paths, network services, and external side effects remain outside the
check's isolation. Clocks, randomness, and scheduling are not perturbed or
controlled. Pre-existing generated files outside `.struo` remain in the
workspace snapshot. Only use this option when repeated execution of the whole
build is authorized.

Reports and trial artifacts remain under `.struo/verifications/<ID>/` and are
not published automatically. See
[Inspect Retained Trials](/guide/scripted/caching#inspect-retained-trials) for
the directory layout and inspection workflow, or the
[verification report reference](/reference/mcp/#verification-reports) for
machine-readable fields.

## Connect Targets With Dependencies

A dependency says that one target consumes the typed output of another. The
following target can live beside `AppCore` in the same manifest:

```toml
[[target]]
name = "App"
kind = "apple_application"
srcs = ["AppSources/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
bundle_id = "dev.struo.App"
minimum_os = "17.0"
families = ["iphone"]
```

`./AppCore` resolves from the package that owns the manifest. `../` moves to a
parent package. References without either prefix resolve from the workspace
root.

Struo validates each dependency against the contract declared by the target
kind. This catches incompatible edges before a compiler or runner starts.

Restrict a target when only selected packages should consume it:

```toml
[[target]]
name = "InternalSupport"
kind = "rust_library"
visibility = ["package:apps/ios", "subtree:tests"]
```

An empty visibility list is public. `private` allows only the same package,
`package:` grants one package, `subtree:` grants a package hierarchy, and an
exact target grants one consumer. `struo query validate-workspace` reports an
attribute-scoped repair when a dependency crosses that boundary.

## Capabilities Become Actions

A capability is an operation a target exposes:

- [`build`](/reference/cli/build) materializes an artifact.
- [`run`](/reference/cli/run) builds required outputs and starts the target.
- [`test`](/reference/cli/test) builds and executes a test target.

The target kind turns a capability into one or more actions. Each action
declares its executable, arguments, inputs, outputs, environment, platform
requirements, and cache policy. Build actions can replay from cache when their
declared inputs match. Launch and device-test actions can opt out of replay
when each invocation must happen again.

The command surface stays the same across ecosystems:

```sh
struo build apps/ios/AppCore
struo run apps/ios/App
struo test apps/ios/AppTests
struo lint quality/swift
```

Ask `struo query capabilities <target>` which of these operations a target
supports instead of guessing from its kind.

When a workspace has more than one test target, continue with
[Testing and scheduling](/guide/graph/testing). It explains conservative
affected selection, exact unit requests, dynamic workers, current ecosystem
coverage, and project-local scripted test adapters.

Use [Linting](/guide/graph/linting) for cacheable, normalized findings from
the analyzers used by each supported ecosystem.

## Choose an Ecosystem

A target's `kind` connects it to a typed contract for a language or platform.
The built-in ecosystem guides continue from the concepts above with runnable,
ecosystem-specific examples:

- [Apple](/guide/graph/apple)
- [Android](/guide/graph/android)
- [C and C++](/guide/graph/c)
- [Bazel](/guide/graph/bazel)
- [CMake](/guide/graph/cmake)
- [Container Images](/guide/graph/containers)
- [Elixir](/guide/graph/elixir)
- [Kotlin](/guide/graph/kotlin)
- [Go](/guide/graph/go)
- [Rust](/guide/graph/rust)
- [Zig](/guide/graph/zig)

The [Ecosystems guide](/guide/graph/ecosystems) compares these choices and
helps you decide when a typed target is a better fit than a script.

## Start From A Native Project

Struo can recognize supported native workspace descriptions before a package has
an explicit target. A native integration supplies an ephemeral seed target, then
the seed's ordinary resolver derives the detailed typed graph from native
metadata.

Before loading a project, follow its ecosystem guide to make the native
lockfile and locked dependency sources available. Detection only reads marker
names, but graph loading may run the native resolver and require those sources.

Inspect the graph Struo derived from the workspace:

```sh
struo query workspace
struo query targets
```

Normal build, run, and test commands can use the derived graph immediately.
Discovery does not write `struo.toml`; the native project description and
lockfile remain authoritative for dependencies, products, tests, and releases.
When discovery produces one buildable workspace root, `struo build` selects it
without requiring a target argument. `struo test` runs the first-party test
targets rooted in that workspace. Use `struo test --all` to include test targets
from its complete resolved dependency graph.

A discovered image is a direct target, so pass its name to `struo build`. See
[Container Images](/guide/graph/containers) for commands that build an existing
Dockerfile without a `struo.toml` file.

The native seed can remain the only target, or it can live beside explicit
targets for exceptional build boundaries. Keep manifest data in `struo.toml`
and introduce a project Starlark module only when reusable behavior is missing
from the built-in target kinds.

An explicit Struo target for the same target kind takes precedence in its
package. Unrelated targets do not hide a native integration in the same package or
elsewhere in the workspace.

An Xcode project checked in beside the repository root is recognized the same
way. Its seed resolves every native target into Apple applications,
frameworks, libraries, resource bundles, and test bundles, so a repository with
no `struo.toml` at all can be queried and built directly:

```sh
struo query targets
struo build MyApp
```

[Xcode Projects](/guide/graph/apple/xcode) walks through the full flow,
including workspaces with several projects, build configurations, and current
limitations.

A Swift Package Manager workspace is recognized from `Package.swift`. Its
native seed lowers first-party package targets into the existing Apple target
kinds, without requiring an `struo.toml` file:

```sh
struo query targets
struo build MyLibrary
```

The [Swift Packages](/guide/graph/swift-packages) guide covers native package
workspaces, locked external package graphs, and network policy.

Every built-in target kind also ships a complete starter with manifests and
source files. Discover the available slugs, then materialize one directly:

```sh
struo query target-kinds
struo edit materialize-example rust_binary rust-binary-minimal
struo build crates/hello/hello
```

Use `struo query example <kind> <slug> --format json` when a caller needs to
inspect and adapt the files before writing them. Contributors can run
`mise run examples:verify-portable` to materialize and execute every portable
starter against the current release build.

## Select Configuration-Specific Values

Some attributes accept `select`, which chooses a value from the active target
configuration. For example, an Apple library can choose a framework by
platform without duplicating the target:

```toml
[target.attrs]
sdk_frameworks = { select = { ios = ["UIKit"], macos = ["AppKit"] } }
```

The root manifest can describe a target platform independently of the host:

```toml
[workspace.configuration]
os = "linux"
arch = "arm64"
tokens = ["release"]
```

`struo query workspace` returns the normalized operating system, architecture,
and ordered selection tokens used by dependency and attribute selection. The
target kind schema identifies configurable attributes and any additional
tokens meaningful for that ecosystem. Attributes marked `implemented: false`
are compatibility fields that validation rejects until the target kind gives
them behavior.

## Run Supported Targets

Some artifacts need a simulator, device, or service after they build. Target
kinds that own this behavior expose the `run` capability directly. Apple and
Android application targets, for example, can build, install, and launch the
application through the same command:

```sh
struo query capabilities apps/mobile/App
struo run apps/mobile/App
```

Check the target's capabilities before running it. The ecosystem guide and
target kind reference explain any required simulator, device, or host setup.

## Validate Shared Mobile Code

The shared-code target kinds expose the `native-mobile-shared-code-e2e`
starter. It wires an Android application to Swift and Rust native libraries,
and an Apple application to a Kotlin/Native framework plus the same Rust
mobile library target. The Apple application calls both shared
implementations. The Android application loads the Swift and Rust libraries
and calls both through the
[Java Native Interface](https://developer.android.com/training/articles/perf-jni).
The Swift target statically links its standard library and packages the C++
shared runtime from the selected toolchain.

## Extend the Graph With Local Modules

Use a local module when a project needs a typed target kind that is not built
in. Root `struo.toml` files can load checked-in
[Starlark](https://starlark-lang.org/) modules:

```toml
[modules]
paths = ["modules/*.star"]
```

Public symbols in those files become target kinds and use the same schema,
dependency, capability, and validation surfaces as built-in kinds. Confirm
that a local kind loaded successfully before declaring targets that use it:

```sh
struo query target-kinds
struo query schema my_target_kind
```

Only the root manifest can declare `[modules]`. Paths resolve from the
workspace root and load in sorted order. See the
[Modules reference](/reference/modules/) when authoring a target kind.

## Give Agents Read Access First

The [Model Context Protocol](https://modelcontextprotocol.io/) server can
expose graph inspection without execution. An agent can discover targets,
query schemas, and inspect target kind contracts without being allowed to
change workspace state:

```sh
struo mcp
```

Editing and running are side-effectful. They can change manifests, build code,
write outputs, install software, or launch a process, so Struo only advertises
state-changing tools when the server is started with an explicit opt-in:

```sh
struo mcp --allow-run
```

With that opt-in, agents can edit manifests, run tests, or call
`struo_build_target`, `struo_run_target`, or `struo_start_target` with the same
target id the command-line interface accepts. The start call returns a runtime
session id immediately, then the agent can use `struo_runtime_status`,
`struo_runtime_logs`, and `struo_stop_runtime` to follow or stop the run. Without
it, the Model Context Protocol surface remains read-only and state-changing
tools are not listed.

For a complete creation and verification loop, including graph-wide validation,
annotated scripts, output checks, and project memory, see
[Coding harnesses](/guide/harness).

See the [Model Context Protocol tools reference](/reference/mcp/tools) for
the available operations.
