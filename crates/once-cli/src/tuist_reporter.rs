//! HTTP summary reporter for the Tuist cache provider.
//!
//! Once already reaches Tuist over `TuistAuth` for the cache endpoint
//! resolution and REAPI credentials. That same token authorizes a
//! lightweight summary post to the Tuist server after each `once exec`
//! completes. The Tuist dashboard uses these summaries to surface a
//! project's cache hit ratio, most recent exec cycles, exit codes, and
//! wall-clock durations.
//!
//! The reporter is a strict best-effort side channel: it never blocks the
//! CLI's exit code, is skipped when the cache provider is not Tuist, and
//! swallows every network error. The full streaming ingest path
//! (`once_events_client::EventClient`) is unchanged; this module supplies
//! the minimum insight surface for deployments that do not yet run the
//! gRPC ingest.
//!
//! Behavior:
//! - Enabled when the resolved cache provider is Tuist AND the workspace
//!   or user config identifies an `account/project` scope.
//! - Skipped silently if the Tuist auth token is not available (unlogged
//!   users, agent contexts).
//! - The POST is wrapped in a 2 second timeout; a slow or unreachable
//!   server never delays the CLI.

use std::env;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use once_cas::{TuistAuth, TuistCacheConfig, TUIST_OAUTH_CLIENT_ID_ENV};
use once_core::{CacheState, Xdg};
use serde::Serialize;
use tokio::time::timeout;

use crate::cache_provider::{credentials_root, resolve_config, ResolvedCacheProviderConfig};

/// Environment variable that opts a run out of Tuist reporting even when
/// the cache provider is Tuist. Useful for CI runs where the operator
/// wants cache traffic but not the summary.
pub const REPORTER_DISABLE_ENV: &str = "TUIST_ONCE_DISABLE_REPORTING";

/// Environment variable that overrides the timeout for the summary POST.
pub const REPORTER_TIMEOUT_MS_ENV: &str = "TUIST_ONCE_REPORTER_TIMEOUT_MS";

const DEFAULT_REPORTER_TIMEOUT: Duration = Duration::from_secs(2);
const REPORT_PATH: &str = "api/projects";

/// The context a caller passes when the exec cycle finishes.
#[derive(Debug, Clone)]
pub struct InvocationReport {
    /// Deterministic per-invocation identifier. Callers commonly derive
    /// this from a `UUIDv7` so it is both unique and time-ordered.
    pub invocation_id: String,
    /// Command that produced this invocation: `exec`, `run`, or a script
    /// adapter surface. Stored verbatim.
    pub command: String,
    /// Full argv the user typed. Trimmed on the server side to a bounded
    /// number of entries; passing the raw list keeps the caller simple.
    pub argv: Vec<String>,
    /// Working directory the wrapped program ran under, relative to the
    /// workspace root. Optional; `None` when the wrapped program
    /// inherited the process's cwd.
    pub cwd: Option<String>,
    /// Action digest produced from the resolved input closure.
    pub action_digest: String,
    /// Whether the action came from the action cache.
    pub cache: CacheOutcome,
    /// Whether the wrapped program (or the cached record of it)
    /// terminated cleanly.
    pub status: RunStatus,
    /// Exit code as reported by the action.
    pub exit_code: i32,
    /// Wall-clock duration of the whole exec cycle in milliseconds.
    pub duration_ms: u64,
    /// Unix epoch ms when the CLI began the exec cycle.
    pub started_at_ms: i64,
    /// Unix epoch ms when the CLI finished the exec cycle.
    pub finished_at_ms: i64,
    /// Git branch, resolved best-effort from the workspace.
    pub git_branch: String,
    /// Git commit sha, resolved best-effort from the workspace.
    pub git_commit_sha: String,
    /// Whether the invocation ran on CI.
    pub is_ci: bool,
    /// Remote execution provider used, if any (`microsandbox`, `daytona`).
    pub remote_execution: Option<String>,
    /// Host operating system, `std::env::consts::OS`.
    pub os: String,
    /// Host architecture, `std::env::consts::ARCH`.
    pub arch: String,
    /// Once CLI version.
    pub once_version: String,
    /// Absolute workspace root, for cross-machine correlation.
    pub workspace: String,
}

/// Simple copy of the cache states we surface upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "Bypass is reserved for callers that skip the cache"
)]
pub enum CacheOutcome {
    Hit,
    Miss,
    Bypass,
}

impl CacheOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Bypass => "bypass",
        }
    }
}

impl From<CacheState> for CacheOutcome {
    fn from(state: CacheState) -> Self {
        match state {
            CacheState::Hit => Self::Hit,
            CacheState::Miss => Self::Miss,
        }
    }
}

/// Success/failure classification for the wrapped run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Success,
    Failure,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

impl From<i32> for RunStatus {
    fn from(exit_code: i32) -> Self {
        if exit_code == 0 {
            Self::Success
        } else {
            Self::Failure
        }
    }
}

/// Fire the invocation report. Never returns an error. Resolves the
/// cache config from `workspace`/`xdg` on the caller's behalf; when the
/// resolved provider is Tuist AND the workspace or user config identifies
/// an `account/project` scope, the summary is posted.
pub async fn report(workspace: &Path, xdg: &Xdg, report: InvocationReport) {
    if env::var_os(REPORTER_DISABLE_ENV).is_some() {
        tracing::debug!("Tuist reporting disabled via {REPORTER_DISABLE_ENV}");
        return;
    }
    let resolved = match resolve_config(workspace, xdg) {
        Ok(config) => config,
        Err(error) => {
            tracing::debug!(%error, "Tuist reporter could not resolve cache config");
            return;
        }
    };
    let ResolvedCacheProviderConfig::Tuist(config) = resolved else {
        return;
    };
    let Some((account, project)) = scoped_handles(&config) else {
        tracing::debug!(
            provider = %config.provider_name,
            "skipping Tuist report because the cache config has no project handle"
        );
        return;
    };

    if let Err(error) = send(&config, xdg, account, project, &report).await {
        tracing::debug!(%error, "Tuist invocation report failed");
    }
}

/// Same as [`report`] but returns the [`Result`] for tests.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "surfaced for integration tests that assert error paths"
)]
pub async fn try_report(
    cache_config: &ResolvedCacheProviderConfig,
    xdg: &Xdg,
    report: &InvocationReport,
) -> Result<()> {
    let ResolvedCacheProviderConfig::Tuist(config) = cache_config else {
        anyhow::bail!("reporter requires a Tuist cache provider");
    };
    let Some((account, project)) = scoped_handles(config) else {
        anyhow::bail!("Tuist report requires an account/project scope");
    };
    send(config, xdg, account, project, report).await
}

fn scoped_handles(config: &TuistCacheConfig) -> Option<(&str, &str)> {
    let account = config.account.as_deref()?;
    let project = config.project.as_deref()?;
    Some((account, project))
}

async fn send(
    config: &TuistCacheConfig,
    xdg: &Xdg,
    account: &str,
    project: &str,
    payload: &InvocationReport,
) -> Result<()> {
    let auth_token = auth_token(config, xdg)?;
    let ingest_base = resolve_events_base(&config.url).await;
    let url = format!("{ingest_base}/{REPORT_PATH}/{account}/{project}/once/invocations");
    let body = ReporterPayload::from(payload);

    let timeout_duration = configured_timeout();
    let client = reqwest::Client::builder()
        .timeout(timeout_duration)
        .build()?;

    let request = client
        .post(&url)
        .bearer_auth(auth_token)
        .header("content-type", "application/json")
        .json(&serde_json::json!({ "events": [body] }));

    match timeout(timeout_duration, request.send()).await {
        Ok(Ok(response)) => {
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("Tuist rejected the invocation report ({status}): {body}");
            }
            Ok(())
        }
        Ok(Err(source)) => Err(source.into()),
        Err(_) => anyhow::bail!("Tuist invocation report timed out after {timeout_duration:?}"),
    }
}

/// Ask the server which endpoint should carry event ingestion. Falls back to
/// `config_url` when discovery is not available or does not advertise an
/// events endpoint - older servers, self-hosted deployments on the
/// pre-discovery release, or a temporary outage all end up posting to the
/// server directly, which is what the reporter did before discovery existed.
async fn resolve_events_base(config_url: &str) -> String {
    let trimmed = config_url.trim_end_matches('/').to_string();
    match crate::discovery::fetch(&trimmed).await {
        Ok(Some(discovery)) => discovery
            .events
            .first_url()
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or(trimmed),
        Ok(None) | Err(_) => trimmed,
    }
}

fn auth_token(config: &TuistCacheConfig, xdg: &Xdg) -> Result<String> {
    let auth = TuistAuth::new(credentials_root(xdg), config);
    let _ = env::var(TUIST_OAUTH_CLIENT_ID_ENV);
    auth.token().map_err(std::convert::Into::into)
}

fn configured_timeout() -> Duration {
    env::var(REPORTER_TIMEOUT_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(DEFAULT_REPORTER_TIMEOUT, Duration::from_millis)
}

#[derive(Debug, Serialize)]
struct ReporterPayload<'a> {
    invocation_id: &'a str,
    command: &'a str,
    argv: &'a [String],
    cwd: Option<&'a str>,
    action_digest: &'a str,
    cache: &'a str,
    status: &'a str,
    exit_code: i32,
    duration_ms: u64,
    started_at_ms: i64,
    finished_at_ms: i64,
    git_branch: &'a str,
    git_commit_sha: &'a str,
    is_ci: bool,
    remote_execution: Option<&'a str>,
    os: &'a str,
    arch: &'a str,
    once_version: &'a str,
    workspace: &'a str,
    provider_name: &'a str,
}

impl<'a> From<&'a InvocationReport> for ReporterPayload<'a> {
    fn from(report: &'a InvocationReport) -> Self {
        Self {
            invocation_id: &report.invocation_id,
            command: &report.command,
            argv: &report.argv,
            cwd: report.cwd.as_deref(),
            action_digest: &report.action_digest,
            cache: report.cache.as_str(),
            status: report.status.as_str(),
            exit_code: report.exit_code,
            duration_ms: report.duration_ms,
            started_at_ms: report.started_at_ms,
            finished_at_ms: report.finished_at_ms,
            git_branch: &report.git_branch,
            git_commit_sha: &report.git_commit_sha,
            is_ci: report.is_ci,
            remote_execution: report.remote_execution.as_deref(),
            os: &report.os,
            arch: &report.arch,
            once_version: &report.once_version,
            workspace: &report.workspace,
            provider_name: "tuist",
        }
    }
}

/// Read git branch and commit sha from the workspace, best effort.
/// Returns empty strings when git resolution fails.
pub fn resolve_git_context(workspace: &Path) -> (String, String) {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let sha = run_git(workspace, &["rev-parse", "HEAD"]).unwrap_or_default();
    (branch, sha)
}

fn run_git(workspace: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Cheap CI detection consistent with the Tuist server's `is_ci` field.
pub fn detect_ci() -> bool {
    [
        "CI",
        "GITHUB_RUN_ID",
        "BUILDKITE",
        "GITLAB_CI",
        "BUILD_NUMBER",
    ]
    .iter()
    .any(|name| env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cas::TuistCacheConfig;

    fn config() -> TuistCacheConfig {
        TuistCacheConfig {
            url: "https://tuist.dev".to_string(),
            account: Some("acme".to_string()),
            project: Some("app".to_string()),
            oauth_client_id: None,
            provider_name: "tuist".to_string(),
        }
    }

    #[test]
    fn scoped_handles_reads_account_and_project() {
        let cfg = config();
        assert_eq!(scoped_handles(&cfg), Some(("acme", "app")));
    }

    #[test]
    fn scoped_handles_returns_none_without_project_scope() {
        let mut cfg = config();
        cfg.project = None;
        assert!(scoped_handles(&cfg).is_none());
    }

    #[test]
    fn cache_outcome_from_cache_state() {
        assert_eq!(CacheOutcome::from(CacheState::Hit).as_str(), "hit");
        assert_eq!(CacheOutcome::from(CacheState::Miss).as_str(), "miss");
    }

    #[test]
    fn run_status_from_exit_code() {
        assert_eq!(RunStatus::from(0).as_str(), "success");
        assert_eq!(RunStatus::from(1).as_str(), "failure");
        assert_eq!(RunStatus::from(-1).as_str(), "failure");
    }

    #[test]
    fn reporter_payload_serializes_report_shape() {
        let report = InvocationReport {
            invocation_id: "01JT-serialize".to_string(),
            command: "exec".to_string(),
            argv: vec!["bash".to_string(), "scripts/build.sh".to_string()],
            cwd: Some(".".to_string()),
            action_digest: "cafebabe/64".to_string(),
            cache: CacheOutcome::Miss,
            status: RunStatus::Success,
            exit_code: 0,
            duration_ms: 1_500,
            started_at_ms: 1_700_000_000_000,
            finished_at_ms: 1_700_000_001_500,
            git_branch: "main".to_string(),
            git_commit_sha: "deadbeef".to_string(),
            is_ci: false,
            remote_execution: None,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            once_version: "0.55.0".to_string(),
            workspace: "/repos/mise".to_string(),
        };
        let payload = ReporterPayload::from(&report);
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["invocation_id"], "01JT-serialize");
        assert_eq!(value["cache"], "miss");
        assert_eq!(value["status"], "success");
        assert_eq!(value["provider_name"], "tuist");
        assert_eq!(
            value["argv"],
            serde_json::json!(["bash", "scripts/build.sh"])
        );
    }
}
