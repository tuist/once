# Cache and Execution

Fabrik caches at the action boundary. An action has argv, env, cwd, an input digest, declared outputs, timeout, and a resource request.

## Cache Layers

- The action cache maps an action digest to result metadata.
- The CAS stores stdout, stderr, and declared output contents.
- Cache hits restore declared outputs before downstream work runs.

## Cacheable Work

These are cacheable today:

- Granular Rust build actions.
- Rust test binary execution.
- `cargo.binary` actions.
- Apple Swift compile, archive, framework, and macOS executable actions.
- `apple.ios_app` build actions.
- `task` targets with `cache = true`.
- Literal `fabrik exec` commands, unless the command or declared inputs change.

These are intentionally uncached:

- `apple.ios_app` simulator install and launch.
- `task` targets with `cache = false`.

## Resource Bounds

Every action has a `ResourceRequest`:

- `cpu_slots`
- `memory_bytes`

The local runner uses those requests to avoid oversubscribing the machine. Remote execution should use the same model when the REAPI client is added.

## Remote Execution Direction

Fabrik should use Bazel's Remote Execution API at the wire boundary: CAS, action cache, and execution service. The scheduler should keep separate budgets for local work, remote queue depth, output downloads, and speculative prefetch.
