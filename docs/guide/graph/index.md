# Graph

The Once graph describes the named parts of a workspace and what Once can do
with them. Start with one target, inspect it, and build it. Add dependencies or
more specialized target kinds only when the project needs them.

## Start With One Target

Targets live in package-level `once.toml` files. This example declares an
Apple library in `apps/ios/once.toml`:

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
once query targets
once query capabilities apps/ios/AppCore
once query schema apple_library
```

The first command lists the workspace. The second shows what `AppCore` can do.
The third explains which attributes, dependencies, outputs, and capabilities
an `apple_library` accepts.

Build the same target:

```sh
once build apps/ios/AppCore
```

Outputs are materialized under `.once/out/<target>/`. The
[target kind reference](/reference/prelude/) lists the exact output groups for
each kind.

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
bundle_id = "dev.once.App"
minimum_os = "17.0"
families = ["iphone"]
```

`./AppCore` resolves from the package that owns the manifest. `../` moves to a
parent package. References without either prefix resolve from the workspace
root.

Once validates each dependency against the contract declared by the target
kind. This catches incompatible edges before a compiler or runner starts.

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
once build apps/ios/AppCore
once run apps/ios/App
once test apps/ios/AppTests
```

Ask `once query capabilities <target>` which of these operations a target
supports instead of guessing from its kind.

When a workspace has more than one test target, continue with
[Testing and scheduling](/guide/graph/testing). It explains conservative
affected selection, exact unit requests, dynamic workers, current ecosystem
coverage, and project-local scripted test adapters.

## Choose an Ecosystem

A target's `kind` connects it to a typed contract for a language or platform.
The built-in ecosystem guides continue from the concepts above with runnable,
ecosystem-specific examples:

- [Apple](/guide/graph/apple)
- [Android](/guide/graph/android)
- [C and C++](/guide/graph/c)
- [Elixir](/guide/graph/elixir)
- [Kotlin](/guide/graph/kotlin)
- [Go](/guide/graph/go)
- [Rust](/guide/graph/rust)
- [Zig](/guide/graph/zig)

The [Ecosystems guide](/guide/graph/ecosystems) compares these choices and
helps you decide when a typed target is a better fit than a script.

Every built-in target kind also ships a complete starter with manifests and
source files. Discover the available slugs, then return one starter as
structured data:

```sh
once query target-kinds
once query example apple_library apple-library-minimal --format json
```

Use the starter when you want a complete copyable workspace. Use the guide
when you want to understand how targets connect and which capability to invoke
next.

## Select Configuration-Specific Values

Some attributes accept `select`, which chooses a value from the active target
configuration. For example, an Apple library can choose a framework by
platform without duplicating the target:

```toml
[target.attrs]
sdk_frameworks = { select = { ios = ["UIKit"], macos = ["AppKit"] } }
```

The target kind schema identifies configurable attributes and the tokens that
are meaningful for that ecosystem. Attributes that determine the active
configuration must remain literal so Once can select a branch unambiguously.

## Run Supported Targets

Some artifacts need a simulator, device, or service after they build. Target
kinds that own this behavior expose the `run` capability directly. Apple and
Android application targets, for example, can build, install, and launch the
application through the same command:

```sh
once query capabilities apps/mobile/App
once run apps/mobile/App
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
in. Root `once.toml` files can load checked-in
[Starlark](https://starlark-lang.org/) modules:

```toml
[modules]
paths = ["modules/*.star"]
```

Public symbols in those files become target kinds and use the same schema,
dependency, capability, and validation surfaces as built-in kinds. Confirm
that a local kind loaded successfully before declaring targets that use it:

```sh
once query target-kinds
once query schema my_target_kind
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
once mcp
```

Editing and running are side-effectful. They can change manifests, build code,
write outputs, install software, or launch a process, so Once only advertises
state-changing tools when the server is started with an explicit opt-in:

```sh
once mcp --allow-run
```

With that opt-in, agents can edit manifests, run tests, or call
`once_build_target`, `once_run_target`, or `once_start_target` with the same
target id the command-line interface accepts. The start call returns a runtime
session id immediately, then the agent can use `once_runtime_status`,
`once_runtime_logs`, and `once_stop_runtime` to follow or stop the run. Without
it, the Model Context Protocol surface remains read-only and state-changing
tools are not listed.

For a complete creation and verification loop, including graph-wide validation,
annotated scripts, output checks, and project memory, see
[Coding harnesses](/guide/harness).

See the [Model Context Protocol tools reference](/reference/mcp/tools) for
the available operations.
