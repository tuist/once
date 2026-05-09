# Build Script Cfg

This example represents a lower-level granular Rust graph where a
`build.rs` program controls how a dependent library is compiled.

The build script reads `flag.txt`. When the file contains `enabled`,
the script emits `cargo::rustc-cfg=build_script_enabled`. The
`cfg-lib` library uses that cfg to choose its output, and the `app`
binary prints the selected value.

This scenario is useful for checking that build-script directives are
part of the explicit Fabrik graph and that cache invalidation flows
from declared build-script inputs to dependent Rust targets.

Run it from the repository root:

```sh
mise exec -- target/release/fabrik build //examples/rust/granular/build-script-cfg:app
./.fabrik/out/examples/rust/granular/build-script-cfg/app
```
