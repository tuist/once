# Deferred action planning

Status: implemented for compiler-driven Apple module discovery.

## Motivation

Bazel's rules_swift 4.0.1 makes explicit modules opt-in and adds strict Swift
dependency checking. Once needs compiler-derived module actions while retaining
native Xcode and Swift package declarations. A source import parser cannot
reproduce compiler conditionals, Clang module variants, or textual interface
dependencies reliably.

## Contract

Target kinds declare an ordinary cacheable dependency scan followed by
`expand_actions`. The runner materializes the planning inputs and invokes an
exported Starlark callback in the command's existing analysis engine, under
resource admission. The callback declares ordinary actions or returns a
structured diagnostic. It must produce every promised output and cannot
recursively expand. Rust owns scheduling and validation, not compiler-specific
interpretation.

The Apple planner interprets the compiler scan, orders discovered modules,
preserves scanner-generated compile commands, declares workspace headers and
interfaces, emits a module map, and disables implicit modules on the original
compile. Host compiler, development-kit, Clang resource and plugin contents
partition the module namespace. Existing action locks and cache keys reuse
identical module builds across targets.
Scans use private, cleared scratch paths. Shared module output namespaces include
normalized compiler commands, source-content digests, dependency identities and
environment, rather than relying only on the compiler's context hash.
Only declared module outputs are remapped. Auxiliary inputs inside the scanner's
temporary cache are rejected with a structured diagnostic, since a cached scan
does not restore that scratch directory. Absolute scanner output paths are
normalized before remapping.

Consecutive actions opting out of prior-action identity can run in bounded
batches when their declared filesystem accesses do not conflict. Default
ordered actions, cleanup conflicts, signing mutations, and uncacheable actions
remain barriers. This does not change dependency-target readiness.

## Native dependency checking

Direct source imports reported by the compiler are checked against direct
module providers. Xcode lowering retains declared edges before adding inferred
imports, so inference cannot silently satisfy strict checking. Defaults remain
compatible; strict checks require explicit modules and compiler support.

## Cache and query limitations

Initial queries expose planning barriers, not a speculative expanded graph.
Execution evidence records child actions. Whole-target shortcut records are
disabled for deferred targets until discovered inputs and planner observations
can be persisted safely; child action caching remains enabled. Host paths in
scanner commands limit relocation and remote execution. A content identity is
not toolchain distribution or a proof that all undeclared host reads are absent.

## Validation

Compiler-free tests cover scan interpretation, ordering, dependency diagnostics,
callback contracts, and scheduler conflicts. A small native acceptance fixture
covers module sharing, warm reuse, deleted-output restoration, header edits,
Swift package discovery, and Xcode settings. It reuses the release binary and
existing test jobs instead of compiling a separate test driver.
Bridging headers, testable application imports, and framework consumers also
run through the real compiler. Scans name their output file explicitly and
disable implicit bridging-header precompilation, avoiding driver-dependent
output streams and temporary inputs outside the graph.

## Remaining parity work

Artifact-ready cross-target scheduling, relocatable scan plans, packaged Apple
toolchains, and stricter standalone C-family imports remain separate changes.
Broader signing, device testing and product packaging parity cannot be inferred
from explicit module support. Symbolic macros and Bzlmod are Bazel authoring
and dependency-management mechanisms, not formats Once needs to adopt.

## Sources

- [rules_swift 4.0.1](https://github.com/bazelbuild/rules_swift/releases/tag/4.0.1)
- [Explicit modules](https://github.com/bazelbuild/rules_swift/blob/main/doc/explicit_modules.md)
- [Hermetic Swift toolchains](https://github.com/bazelbuild/rules_swift/pull/1630)
- [rules_apple 5.0.0](https://github.com/bazelbuild/rules_apple/releases/tag/5.0.0)
- [Apple symbolic macros](https://github.com/bazelbuild/rules_apple/pull/2933)
