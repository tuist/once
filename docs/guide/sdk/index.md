# SDKs

Once SDKs expose cache primitives for applications and tools that want
to embed Once directly. They are intentionally narrower than the CLI:
SDKs read and write blobs, action results, and cache metadata. Script
execution, graph loading, runtime sessions, and provider configuration
stay behind the CLI and workspace configuration.

Use an SDK when your program already owns execution and only needs the
cache substrate. Use `once exec`, `once run`, `once build`, or
`once test` when you want Once to construct and execute actions for you.

## Choose An SDK

| SDK | Use When |
| --- | --- |
| [Rust](/guide/sdk/rust) | You are embedding Once in Rust tools, services, or repository automation. |
| [Swift](/guide/sdk/swift) | You are integrating Once cache access in Apple platform tooling. |
| [Ruby](/guide/sdk/ruby) | You are writing Ruby automation, gems, or repository scripts. |
| [JavaScript](/guide/sdk/javascript) | You are writing Node.js tools or JavaScript automation. |

## Shared Model

Every SDK follows the same model:

- `Cache` opens the default local cache using XDG conventions.
- Blob APIs store and read content-addressed bytes.
- Action-result APIs associate an action digest with exit code, stdout,
  stderr, and declared output digests.
- SDKs do not execute commands. Callers define the action digest, run
  their own work, and decide which outputs belong in the cached result.

The language guides show installation details and API shapes for each
SDK.
