# Fabrik Docs

> [!WARNING]
> Fabrik is beta software. The docs describe the current implementation, not a stable compatibility contract.

## Supported Today

- [Rust](rust.md): granular `rust.library`, `rust.binary`, `rust.test`, `rust.proc_macro`, plus the `cargo.binary` escape hatch.
- [Apple](apple.md): build and launch a Swift iOS simulator app bundle.
- [Rules and targets](rules.md): canonical `[[target]] rule = "..."` model and Starlark rule-authoring direction.
- [Tasks](tasks.md): checked-in command targets that run through `fabrik run`.
- [Runtime sessions](runtime.md): headless runtime target inspection and control model.
- [Cache and execution](cache-and-execution.md): action cache, CAS, declared outputs, uncached runtime work, and resource bounds.

## Target IDs

CLI target ids are project-root relative by default:

```sh
fabrik build examples/apple/macos/cli/hello
```

Use `./` or `../` when you want the argument resolved from your
current directory instead:

```sh
cd examples/apple/macos/cli
fabrik build ./hello
```

Build-file deps are resolved from the declaring `fabrik.toml` by
default. `Greeter` means another target in the same file's directory,
`../shared/Logging` is relative to that directory, and
`shared/Logging` is project-root relative.

## Design Notes

- [Agent-native features](agent-native.md)
- [Design](design.md)
- [Roadmap](roadmap.md)

## Examples

- `examples/rust/granular/basic-app`: Rust library, binary, and test target.
- `examples/rust/granular/build-script-cfg`: granular Rust build script cfg propagation.
- `examples/apple/macos/cli`: Swift library plus macOS command-line app.
- `examples/apple/macos/tuist-shaped-swift`: a Tuist-shaped Swift module graph.
- `examples/apple/ios/simulator-app`: SwiftUI iOS simulator app.
