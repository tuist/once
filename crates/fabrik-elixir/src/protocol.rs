//! Wire protocol shared between `fabrik elixir-compile` (client) and
//! the long-lived compile daemon spawned by `fabrik elixir-daemon`.
//!
//! Format: line-delimited JSON over a unix domain socket. Each message
//! is one JSON object on its own line. Paths in requests are workspace
//! relative; the daemon resolves them against `cwd`.

use serde::{Deserialize, Serialize};

/// Current wire-format version. Bump on breaking schema changes; the
/// daemon and clients agree by comparing this field before processing.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompileRequest {
    pub v: u32,
    pub id: u64,
    /// Absolute workspace root. Every other path resolves against this.
    pub cwd: String,
    /// Workspace-relative output `.ebin` directory.
    pub out: String,
    /// Workspace-relative dep `.ebin` directories, prepended to the
    /// BEAM code path for the duration of this compile.
    #[serde(default)]
    pub pa: Vec<String>,
    /// Workspace-relative `.ex` (or `.exs`) source files to compile.
    pub srcs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompileResponse {
    pub v: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True when the daemon refused the job for a transient,
    /// non-correctness reason (queue saturation, overload). Compile
    /// failures keep this `false` because re-running them won't help.
    /// Clients that see this should fall back to direct `elixirc`
    /// rather than retry blindly against the same saturated daemon.
    #[serde(default, skip_serializing_if = "is_default_bool")]
    pub retryable: bool,
}

// Serde's `skip_serializing_if` requires `fn(&T) -> bool`, so this
// signature is fixed even though `bool` is `Copy` and would be cheaper
// to pass by value. Clippy's `trivially_copy_pass_by_ref` doesn't know
// about that contract, so we silence it locally.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_bool(b: &bool) -> bool {
    !*b
}

impl CompileRequest {
    pub fn new(id: u64, cwd: String, out: String, pa: Vec<String>, srcs: Vec<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            cwd,
            out,
            pa,
            srcs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let req = CompileRequest::new(
            42,
            "/abs/ws".into(),
            ".fabrik/out/foo.ebin".into(),
            vec![".fabrik/out/dep.ebin".into()],
            vec!["lib/foo.ex".into(), "lib/bar.ex".into()],
        );
        let line = serde_json::to_string(&req).unwrap();
        let decoded: CompileRequest = serde_json::from_str(&line).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn response_omits_error_when_ok() {
        let r = CompileResponse {
            v: 1,
            id: 1,
            ok: true,
            error: None,
            retryable: false,
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("error"));
        assert!(!line.contains("retryable"));
    }

    #[test]
    fn response_includes_error_when_failed() {
        let r = CompileResponse {
            v: 1,
            id: 1,
            ok: false,
            error: Some("** (CompileError) ...".into()),
            retryable: false,
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains("\"error\":"));
    }

    #[test]
    fn busy_response_carries_retryable_flag() {
        let r = CompileResponse {
            v: 1,
            id: 9,
            ok: false,
            error: Some("queue full".into()),
            retryable: true,
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains("\"retryable\":true"));
        let parsed: CompileResponse = serde_json::from_str(&line).unwrap();
        assert!(parsed.retryable);
    }

    #[test]
    fn empty_pa_is_optional_on_the_wire() {
        // Daemon must accept requests that omit `pa` entirely (no deps).
        let raw = r#"{"v":1,"id":7,"cwd":"/w","out":"o","srcs":["a.ex"]}"#;
        let decoded: CompileRequest = serde_json::from_str(raw).unwrap();
        assert!(decoded.pa.is_empty());
    }

    #[test]
    fn version_field_is_serialised() {
        // Wire-format consumers may inspect `v` before further parsing
        // (e.g. to reject mismatched protocols cleanly), so it must be
        // present even when it equals the current default.
        let req = CompileRequest::new(1, "/w".into(), "o".into(), vec![], vec!["x.ex".into()]);
        let line = serde_json::to_string(&req).unwrap();
        assert!(line.contains("\"v\":1"));
    }
}
