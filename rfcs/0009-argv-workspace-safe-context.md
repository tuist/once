# Request for Comments 0009: Argv Workspace Safe Context

## Status

Draft.

## Motivation

The v1 argv normalization algorithm defined in [RFC 0008][0008] is
defensive-first. Everything outside a small frozen allowlist of tool
and subcommand names hashes to `OpaqueValue`, so a normal invocation
like `once build my-app --features telemetry` reaches the Tuist
dashboard as `once build ⟨opaque⟩ --features ⟨opaque⟩`. The dashboard
renders the opaque positions as `[redacted]` because it has no
plaintext to show.

The defensive-first default was correct for v1: the client had no way
to know which strings were safe, and the RFC explicitly names the
extension it left room for:

> Readable positional values (package names, target names) show up as
> `OpaqueValue` unless the specific literal appears on the allowlist.
> A future v1.x may introduce tool-specific parsers behind a
> per-project setting; the schema shape for `ArgvToken` does not need
> to change to add them.

The current default is now costing us more than it protects. A shared
dashboard for a team run is only useful when a viewer can read the
run: what target ran, which features were on, which profile was
selected. Redacting every one of those turns the run detail into a
"something happened" counter, and every question a viewer would ask
of the dashboard needs the terminal that ran the command.

The protection the current default gives is also narrower than it
looks. Every token this RFC proposes to reclassify as safe is already
present in the workspace manifest checked into git and is therefore
already visible to anyone who can view the dashboard's project — a
target name, a declared feature, a profile listed in `once.toml`.
None of those are secrets on the same trust boundary the dashboard
lives inside.

## Decision

The client-side normalization algorithm is extended with a new
optional input: a **workspace safe context** derived from data that is
already public inside the workspace. Any argv token whose exact value
appears in that context emits `SafeLiteral` instead of `OpaqueValue`
or the value half of a `NamedValue`. All other tokens follow the
existing rules unchanged.

The workspace safe context is opt-out, not opt-in: a workspace that
declares `[reporting] argv_privacy = "strict"` in its root
`once.toml` skips the context and gets the v1 defensive-first
behaviour verbatim. Nothing else about the wire protocol changes.

## Workspace Safe Context

The context is a set of strings, built once per run and cached for
the run's lifetime. It contains:

- **Target labels** exposed by the loaded workspace graph (both the
  fully-qualified `<package>/<name>` label id and the bare `<name>`
  form so `once build my-app` classifies `my-app` verbatim).
- **Declared feature names** from `[[target]]` `attrs.features` and
  the equivalent in ecosystem manifests the graph loader consumed
  (`Cargo.toml` `[features]`, `Package.swift` `traits`, …).
- **Profile names** declared in `[[profile]]` sections of `once.toml`
  and equivalents (`Cargo.toml` `[profile.*]`).
- **Configuration keys** from `[configuration]` sections of
  `once.toml`.

Everything in the context is data already present in the repository
under source control, or in a manifest the repository imports.
Nothing is derived from process environment, filesystem paths outside
the workspace, or user identity.

Two tokens are excluded from the context on principle even when they
match: absolute filesystem paths (leading `/` or a drive letter) and
tokens containing `=` (these still go through `split_named_value`).

The context is not sent to the server. The server continues to
verify each `SafeLiteral` against the frozen allowlist version the
client declared; any `SafeLiteral` outside the allowlist is
quarantined and rewritten to `OpaqueValueHash` on ingest per RFC 0008
§Safe literal allowlist. To keep the server-side check meaningful
while widening the client's reach, the allowlist version stamped in
`RunStarted.safe_literal_allowlist_version` is upgraded to a new
value that authorises the workspace-context extension. The server
enforces the extension by requiring the client's declared version
and rejecting `SafeLiteral` tokens from an older client that would
have hashed them.

## Config Surface

Root `once.toml` gains one optional section:

```toml
[reporting]
argv_privacy = "show"   # default. Emits SafeLiteral for every match.
argv_privacy = "strict" # opts back into the v1 defensive-first default.
```

No other keys are introduced in this RFC. Fine-grained per-token
policies are a follow-up if `strict` and `show` prove insufficient.

## Compatibility

- **Client**: adds one field to the normalization entry point. The
  proto is unchanged. The frozen allowlist file gains a new version
  string but no new content; the workspace-context extension is
  authorised by the version bump alone.
- **Server**: no schema change. The projector already stores
  per-token classes and renders `SafeLiteral` verbatim. The
  allowlist checker is updated once to the new version.
- **Older clients**: unchanged behaviour. Their runs continue to
  emit under the v1 allowlist and render with the same opaque
  positions.
- **Older servers**: reject the new allowlist version they do not
  know. Clients degrade to the v1 model when the server declares an
  older version in `ServerCapabilities.safe_literal_allowlist_version`.

## Non-Goals

- Tool-specific parsing (`-p my-crate` inferring that `my-crate`
  belongs to `-p`) is deferred. `NamedValue` still requires the
  combined `--key=value` form.
- Redaction of tokens that were previously safe (e.g. `--release`)
  is out of scope. The RFC only widens what counts as safe.
- Server-side augmentation of the allowlist per account or project
  is out of scope; the allowlist stays a single Once-owned file.

## Rollout

1. Land the client change behind a new allowlist version.
2. Land the server-side allowlist checker for the new version.
3. Ship both together; older clients keep working under the old
   version and older servers reject the new version, so no run
   silently downgrades or is quarantined.
4. Document the `[reporting] argv_privacy` key in the workspace
   manifest reference.

[0008]: 0008-live-run-event-protocol.md
