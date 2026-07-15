---
prev: false
next: false
---

# C and C++

Once can compile C and C++ sources into object files and archive them as a
static library. This guide starts with the shipped C library example, then
shows how the same target accepts C++ sources.

## Prerequisites

The default toolchain requires `cc` and `ar` on the command search path. A
target with `.cc`, `.cpp`, or `.cxx` sources also requires `c++`:

```sh
cc --version
ar --version
c++ --version
```

Set `compiler`, `archiver`, or `cxx_compiler` when the project should use
different executable paths.

## Declare a Static Library

Create `native/math/once.toml`:

```toml
[[target]]
name = "native_math"
kind = "c_library"
srcs = ["src/*.c"]

[target.attrs]
hdrs = ["include/native_math.h"]
includes = ["include"]
```

Use this source layout:

```text
native/math/
├── once.toml
├── include/
│   └── native_math.h
└── src/
    └── native_math.c
```

`hdrs` publishes the header to dependent targets. `includes` lets the source
and consumers include it as `#include "native_math.h"`.

## Query Before Building

Inspect the target and the complete `c_library` contract:

```sh
once query targets --kind c_library
once query capabilities native/math/native_math
once query schema c_library
```

The target exposes `build`. There is no shipped C or C++ binary or test target,
so this guide does not use `once run` or `once test`.

## Build the Library

Build the static archive:

```sh
once build native/math/native_math
```

On macOS and Linux, the default archive is
`.once/out/native/math/native_math/libnative_math.a`. On Windows, it is
`.once/out/native/math/native_math/native_math.lib`. Compiled objects appear
under `.once/out/native/math/native_math/objects/` and preserve the source
path, which prevents similarly named sources from colliding.

Set `output_name` to change the archive name without changing the target
identifier.

## Add C++ Sources

The same target can compile C and C++ together. Extend its source patterns when
the library gains a C++ implementation:

```toml
[[target]]
name = "native_math"
kind = "c_library"
srcs = ["src/*.c", "src/*.cpp"]

[target.attrs]
hdrs = ["include/native_math.h", "include/native_stats.hpp"]
includes = ["include"]
```

Once selects `cc` for `.c` files and `c++` for `.cc`, `.cpp`, and `.cxx`
files. The C++ compiler is not required for a target that contains only C
sources.

## Connect Native Consumers

Another `c_library` can depend on `./native_math`; it receives the transitive
headers, include directories, definitions, static libraries, dynamic
libraries, linker flags, and data declared by the dependency.

Zig targets consume the same contract directly. For example, a
[`zig_binary`](/reference/prelude/zig_binary) can list this library in `deps`
and receive its headers and link inputs. Apple link targets can consume the
static archive through the native linkable contract. For Android packaging,
the target can propagate prebuilt dynamic libraries when `android_abi`
identifies their
[Application Binary Interface](https://developer.android.com/ndk/guides/abis).

Header-only targets are valid. Omit `srcs` and publish `hdrs` or
`header_globs`; the target then propagates its compile context without
producing an archive.

## Target Reference and Limitations

The [`c_library` reference](/reference/prelude/c_library) documents compiler
selection, public headers, include directories, definitions, compiler and
linker options, prebuilt dynamic libraries, data, dependencies, and the
`library` output group.

The shipped target kind builds static libraries only. It does not link a C or
C++ executable, build a shared library from source, or provide a native test
runner. Use a consuming ecosystem target when it owns the final executable or
shared artifact.

## Next

Continue with [Zig](/guide/graph/zig) when a Zig executable should consume the
library. Otherwise, connect the archive to the Apple or Android target that
owns the final product, then query that consumer before building it.
