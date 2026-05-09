# Fabrik Docs

> [!WARNING]
> Fabrik is beta software. The docs describe the current implementation, not a stable compatibility contract.

## Supported Today

- [Rust](rust.md): granular `rust.library`, `rust.binary`, `rust.test`, `rust.proc_macro`, plus the `cargo.binary` escape hatch.
- [Apple and iOS](apple.md): build and launch a Swift iOS simulator app bundle.
- [Tasks](tasks.md): checked-in runtime tasks that run through `fabrik run`.
- [Cache and execution](cache-and-execution.md): action cache, CAS, declared outputs, uncached runtime work, and resource bounds.

## Design Notes

- [Agent-native features](agent-native.md)
- [Design](design.md)
- [Roadmap](roadmap.md)

## Examples

- `examples/rust/granular/basic-app`: Rust library, binary, and test target.
- `examples/rust/granular/build-script-cfg`: granular Rust build script cfg propagation.
- `examples/apple/ios/simulator-app`: SwiftUI iOS simulator app.
