# Graph

The Once graph is a typed build model that sits above the cacheable
script ramp. When teams need richer relationships than scripts can
express, the same work moves into typed graph targets that carry
schemas, dep edges, capabilities, and structured diagnostics.

## Where the Graph Fits

Once has three layers:

1. **Script actions**: annotated scripts that `once exec` runs through
   the action cache. See [Scripts](/guide/scripts/).
2. **Script targets**: declared graph targets that wrap a script
   action so it participates alongside typed rules.
3. **Build graph targets**: typed targets validated against a *rule
   schema* (the contract for a kind: which attributes it accepts,
   which providers each dep edge expects, which providers it emits,
   and which capabilities it exposes), carrying typed attributes,
   declared outputs, and structured diagnostics.

Scripts are the migration ramp. Teams move into graph targets when
they need stronger relationships, multiple capabilities, or richer
diagnostics.

## Targets

Targets live in `once.toml` files at the package level:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]
deps = ["./Logging"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
```

Dep references are root-relative by default; `./` and `../` resolve
against the package that owns the manifest.

## Rule Domains

Today the prelude ships rules for one platform domain:

- [Apple](/guide/graph/apple): Swift, Objective-C, C, and C++ static
  libraries with module emission, bridging headers, and modulemap
  generation; framework, application, and XCTest bundle rules are in
  flight.

Per-domain deep dives cover rule schemas, attributes, providers, and
command behavior.

## Capabilities

Each rule declares which capabilities its targets expose:
[`build`](/reference/cli/build), [`run`](/reference/cli/run), and
[`test`](/reference/cli/test). The CLI dispatches on capability, and
every capability runs as a cacheable action through the same substrate
scripts use.

```sh
once query targets
once query schema apple_library
once build apps/ios/AppCore
once run  apps/ios/App
once test apps/ios/AppTests
```

[`query`](/reference/cli/query) commands return structured JSON so
agents and humans can ask what a target can do before any execution
happens.
