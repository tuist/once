# Configurations

A target's behavior can vary by configuration: which platform it targets,
which dependencies it pulls in, or which build mode it uses. Once keeps the
declared graph static while letting its configured projection change between
invocations. The same target identifier can resolve to different dependencies
and actions without editing the manifest.

Configuration plays two roles. It selects which branch of a `select` value a
target uses, and it feeds target-kind analysis with the platform and tokens
the target runs against. The effective configuration is also part of a
target's identity, so two invocations with different configurations never
share outputs or receipts: each lands in its own scoped output directory and
its own action-cache partition.

## Declare workspace defaults

The workspace root manifest fixes the default configuration every invocation
starts from:

```toml
[workspace.configuration]
os = "linux"
arch = "arm64"
tokens = ["release"]
```

Once normalizes common names (`darwin` to `macos`, `arm64` to `aarch64`,
`amd64` to `x86_64`) and derives an ordered set of selection tokens from the
operating system and architecture, then appends the extra `tokens` and
`default`. Inspect the resolved set with `once query workspace`. The
[manifest reference](/reference/manifest) describes the table and
normalization in full.

## Override per invocation

Pass `--config KEY=VALUE` to `once build`, `once lint`, `once run`, or
`once test` to override the workspace defaults for a single invocation.
Recognized keys are `os`, `arch`, and `token`:

```sh
once build apps/Support --config os=ios --config arch=arm64
once build apps/Support --config token=simulator
```

`os` and `arch` replace the workspace values; each `token` is appended to the
selection tokens. When an override changes the operating system or
architecture, Once re-derives the platform tokens for the new target platform
so stale tokens do not survive. Overrides apply on top of the workspace
defaults, and only the resulting values affect target identity, not the flags
that produced them.

Because the effective configuration scopes outputs and receipts, building the
same target under two configurations from one checkout keeps both results:

```sh
once build apps/Support                         # default configuration
once build apps/Support --config token=release  # separate, scoped outputs
```

A later invocation with the same overrides reuses the scoped receipt and
reports a cache hit, while an invocation with no overrides is unaffected.

## Choose values with select

Configurable attributes and dependencies use `select` to vary by token. A
target can choose its dependencies for the configured platform:

```toml
[target.deps.select]
macos = ["./apple_support"]
linux = ["./linux_support"]
default = ["./portable_support"]
```

When several branches could match, the most specific one wins, and `default`
is the fallback. Each ecosystem adds its own tokens and restrictions; see the
"Choose values by configuration" section of the relevant ecosystem guide, such
as [Apple](/guide/graph/apple).
