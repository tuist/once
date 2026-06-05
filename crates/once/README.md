# once

Embeddable SDK for Once cache-aware command execution.

## Rust

Add the crate from this repository:

```toml
[dependencies]
once = { git = "https://github.com/tuist/once" }
```

Run a command with a local cache:

```rust
#[tokio::main]
async fn main() -> once::Result<()> {
    let once = once::Once::new(".", ".once");
    let outcome = once
        .run_command(
            once::Command::new("sh")
                .arg("-c")
                .arg("printf hello")
                .output("dist/app"),
        )
        .await?;

    println!("{} {:?}", outcome.exit_code, outcome.cache);
    Ok(())
}
```

The high-level API is:

- `Once`: reusable runtime bound to a workspace root and cache provider.
- `Command`: builder for argv, environment, workspace-relative cwd, declared outputs, input digest, and timeout.
- `CommandOutcome`: action digest, cache hit or miss state, exit code, and cached output digests.

Lower-level modules are re-exported as `once::core`, `once::cas`, and
`once::frontend` for integrations that need action construction, CAS
access, or manifest parsing.

## Apple

Download `Once.xcframework.zip` from a release and link it from an iOS or
macOS target. Swift can import the C module:

```swift
import Once

let request = """
{
  "workspace_root": ".",
  "cache_root": ".once",
  "argv": ["sh", "-c", "printf hello"],
  "outputs": [],
  "cache_failures": false,
  "stream": false
}
"""

let raw = once_run_command_json(request)!
defer { once_string_free(raw) }
let json = String(cString: raw)
```

All owned strings returned by `once_*` functions must be released with
`once_string_free`.

### FFI responses

FFI functions return UTF-8 JSON:

```json
{ "status": "ok", "value": "..." }
```

or:

```json
{ "status": "error", "message": "..." }
```

`once_run_command_json` returns this value on success:

```json
{
  "action_digest": "64 lowercase hex characters",
  "cache": "hit",
  "exit_code": 0,
  "stdout_digest": "64 lowercase hex characters or null",
  "stderr_digest": "64 lowercase hex characters or null",
  "outputs": {
    "dist/app": "64 lowercase hex characters"
  }
}
```

### Building Locally

On macOS with Xcode installed:

```sh
mise run release:package-xcframework --version 0.0.0
```

The task writes `dist/Once-0.0.0.xcframework.zip` and a SHA-256 file.
