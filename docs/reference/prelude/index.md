# Prelude

The Starlark prelude bundled with `once` declares every built-in rule
kind: its attribute schema, dep edges, providers, capabilities, and
the impl that turns a target into a set of cached actions. The
[`once query schema`](/reference/cli/query) command exposes the same
metadata at the keyboard.

## Apple rules

- [`apple_library`](/reference/prelude/apple_library) — Swift,
  Objective-C, C, and C++ static library
- [`swift_macro`](/reference/prelude/swift_macro) — Swift
  compiler-plugin dylib loaded by `apple_library` deps
- [`apple_framework`](/reference/prelude/apple_framework) — schema
  only, awaiting implementation
- [`apple_application`](/reference/prelude/apple_application) —
  schema only, awaiting implementation
- [`apple_test_bundle`](/reference/prelude/apple_test_bundle) —
  schema only, awaiting implementation
