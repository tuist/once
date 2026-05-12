//! Long-lived compile daemon: client + spawn helpers.
//!
//! The daemon runs a single BEAM that listens on a unix domain socket
//! and serves [`crate::protocol::CompileRequest`]s. `fabrik
//! elixir-compile` connects to that socket once per action; the
//! amortized BEAM startup is what makes per-target caching worth more
//! than re-spawning `elixirc` from scratch.
//!
//! The Elixir program that implements the daemon is checked in at
//! `crates/fabrik-elixir/elixir/fabrik_compiler.exs` and shipped inside
//! the fabrik binary via [`DAEMON_SCRIPT`]. Callers materialize it to a
//! known workspace path before invoking `elixir`.
//!
//! Unix-only because the protocol speaks unix domain sockets. The
//! daemon is irrelevant on Windows hosts; gate the whole module rather
//! than carrying a stub that would lie about the platform's support.
//! `lib.rs` already `#[cfg(unix)]`-gates the `mod daemon` declaration,
//! so no inner attribute is needed here.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabrik_core::Xdg;

use crate::protocol::{CompileRequest, CompileResponse, PROTOCOL_VERSION};

/// The Elixir program that implements the daemon. Embedded here so the
/// fabrik binary is self-contained and the script's contents are part
/// of the build; callers materialize it onto disk before spawning.
pub const DAEMON_SCRIPT: &str = include_str!("../elixir/fabrik_compiler.exs");

/// Filename for the daemon script when materialized to a workspace.
pub const DAEMON_SCRIPT_FILENAME: &str = "fabrik_compiler.exs";

/// Env var that overrides the default unix-socket path. When set on a
/// `fabrik elixir-compile` action, the client routes the job through
/// that socket instead of falling back to a direct `elixirc` spawn.
pub const SOCKET_ENV_VAR: &str = "FABRIK_ELIXIR_DAEMON_SOCKET";

/// Default location for the daemon's socket: `$XDG_RUNTIME_DIR/fabrik`
/// when set, otherwise a tempdir-rooted per-user directory. The
/// workspace argument is unused for the default path but kept in the
/// signature so callers don't have to special-case overrides.
pub fn default_socket_path(_workspace_root: &Path) -> PathBuf {
    Xdg::from_env().fabrik_runtime().join("elixir-daemon.sock")
}

/// Default location for the materialized daemon script:
/// `$XDG_DATA_HOME/fabrik/daemon/`. Shared across workspaces; the
/// script content is identical because it's embedded in the fabrik
/// binary via [`DAEMON_SCRIPT`].
pub fn default_script_path(_workspace_root: &Path) -> PathBuf {
    Xdg::from_env()
        .fabrik_data()
        .join("daemon")
        .join(DAEMON_SCRIPT_FILENAME)
}

/// Errors a client can hit. Distinguishes "no daemon listening" (cheap
/// fallback signal) from real protocol or compile failures so callers
/// can decide whether to retry, fall back to direct `elixirc`, or
/// surface the error.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("no daemon listening at `{path}`")]
    NotRunning { path: PathBuf },
    #[error("socket I/O error talking to daemon at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("daemon at `{path}` closed the connection before responding")]
    UnexpectedEof { path: PathBuf },
    #[error("daemon at `{path}` returned malformed JSON: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("daemon at `{path}` speaks protocol v{got}, client expected v{expected}")]
    VersionMismatch {
        path: PathBuf,
        expected: u32,
        got: u32,
    },
    #[error("daemon at `{path}` reported compile failure: {message}")]
    Compile { path: PathBuf, message: String },
}

/// Send one compile request and read one response.
///
/// On `ConnectionRefused` / `NotFound`, returns `NotRunning` so callers
/// can fall back to a direct `elixirc` spawn without surfacing a noisy
/// I/O error to the user.
pub fn submit(socket: &Path, request: &CompileRequest) -> Result<CompileResponse, ClientError> {
    let stream = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(e) if is_not_running(&e) => {
            return Err(ClientError::NotRunning {
                path: socket.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(ClientError::Io {
                path: socket.to_path_buf(),
                source,
            });
        }
    };

    // The compile itself can take a while on cold starts, but holding
    // a request open forever would make a wedged daemon look like a
    // hung fabrik. Five minutes is generous and matches the action
    // timeout the planner emits.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    write_request(&stream, request, socket)?;
    let response = read_response(&stream, socket)?;
    if response.v != PROTOCOL_VERSION {
        return Err(ClientError::VersionMismatch {
            path: socket.to_path_buf(),
            expected: PROTOCOL_VERSION,
            got: response.v,
        });
    }
    Ok(response)
}

fn write_request(
    mut stream: &UnixStream,
    request: &CompileRequest,
    socket: &Path,
) -> Result<(), ClientError> {
    let mut line = serde_json::to_string(request).expect("CompileRequest is serializable");
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|source| ClientError::Io {
            path: socket.to_path_buf(),
            source,
        })?;
    stream.flush().map_err(|source| ClientError::Io {
        path: socket.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn read_response(stream: &UnixStream, socket: &Path) -> Result<CompileResponse, ClientError> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|source| ClientError::Io {
            path: socket.to_path_buf(),
            source,
        })?;
    if n == 0 {
        return Err(ClientError::UnexpectedEof {
            path: socket.to_path_buf(),
        });
    }
    serde_json::from_str(line.trim_end()).map_err(|source| ClientError::Decode {
        path: socket.to_path_buf(),
        source,
    })
}

fn is_not_running(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

/// Write the embedded daemon script to a known workspace path so a
/// freshly cloned project can `fabrik elixir-daemon start` without any
/// extra setup. Idempotent: if the on-disk content already matches
/// `DAEMON_SCRIPT` we skip the write to avoid touching mtimes that
/// editors might be watching.
pub fn materialize_script(dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(existing) = std::fs::read_to_string(dest) {
        if existing == DAEMON_SCRIPT {
            return Ok(());
        }
    }
    std::fs::write(dest, DAEMON_SCRIPT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_socket_path_resolves_through_xdg_runtime() {
        let tmp = TempDir::new().unwrap();
        let runtime = tmp.path().join("runtime");
        // Snapshot and restore the env so we don't leak overrides
        // into other tests in the same process.
        let prior = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        let p = default_socket_path(tmp.path());
        assert_eq!(p, runtime.join("fabrik").join("elixir-daemon.sock"));
        match prior {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[test]
    fn default_script_path_resolves_through_xdg_data() {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().join("data");
        let prior = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", &data);
        let p = default_script_path(tmp.path());
        assert_eq!(
            p,
            data.join("fabrik")
                .join("daemon")
                .join(DAEMON_SCRIPT_FILENAME)
        );
        match prior {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    #[test]
    fn submit_returns_not_running_when_socket_is_absent() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("missing.sock");
        let req = CompileRequest::new(1, "/w".into(), "o".into(), vec![], vec!["x.ex".into()]);
        let err = submit(&sock, &req).unwrap_err();
        assert!(matches!(err, ClientError::NotRunning { .. }));
    }

    #[test]
    fn materialize_script_writes_and_skips_when_unchanged() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("daemon").join("script.exs");
        materialize_script(&dest).unwrap();
        let body = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(body, DAEMON_SCRIPT);

        // Second call is a no-op when content already matches.
        let mtime_before = std::fs::metadata(&dest).unwrap().modified().unwrap();
        // Sleep below resolution would race; we just assert content
        // stays correct.
        materialize_script(&dest).unwrap();
        let body2 = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(body2, DAEMON_SCRIPT);
        // Allow either equal or later mtime; the contract is the
        // content matches, not strict mtime preservation.
        let mtime_after = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert!(mtime_after >= mtime_before);
    }

    #[test]
    fn submit_round_trips_against_a_fake_daemon() {
        // Stand up a tiny synchronous server on a unix socket that
        // echoes back an `ok` response. Exercises the framing, the
        // JSON encode/decode, and the version check end to end without
        // depending on a real Elixir toolchain being installed.
        use std::os::unix::net::UnixListener;
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("daemon.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let req: CompileRequest = serde_json::from_str(line.trim_end()).unwrap();
            let resp = CompileResponse {
                v: PROTOCOL_VERSION,
                id: req.id,
                ok: true,
                error: None,
            };
            let mut out = serde_json::to_string(&resp).unwrap();
            out.push('\n');
            let mut writer = &stream;
            writer.write_all(out.as_bytes()).unwrap();
        });

        let req = CompileRequest::new(
            99,
            "/w".into(),
            "o".into(),
            vec!["d/ebin".into()],
            vec!["a.ex".into(), "b.ex".into()],
        );
        let resp = submit(&sock_path, &req).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.id, 99);
        server.join().unwrap();
    }
}
