use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

pub(super) fn variable(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub(super) fn ci_run() -> Option<(&'static str, String)> {
    [
        ("github", "GITHUB_RUN_ID"),
        ("gitlab", "CI_PIPELINE_ID"),
        ("bitrise", "BITRISE_BUILD_SLUG"),
        ("circleci", "CIRCLE_WORKFLOW_ID"),
        ("buildkite", "BUILDKITE_BUILD_NUMBER"),
        ("codemagic", "CM_BUILD_ID"),
    ]
    .into_iter()
    .find_map(|(provider, name)| variable(name).map(|id| (provider, id)))
}

pub(super) async fn collect(workspace: &Path) -> Value {
    let mut value = json!({"is_ci": once_events_client::environment::is_ci()});
    if let Some((provider, id)) = ci_run() {
        value["ci_provider"] = provider.into();
        value["ci_run_id"] = id.into();
    }
    let (branch, commit, origin) = tokio::join!(
        git(workspace, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
        git(workspace, &["rev-parse", "HEAD"]),
        git(workspace, &["remote", "get-url", "origin"]),
    );
    for (key, entry) in [
        (
            "git_branch",
            once_events_client::environment::ci_branch().or(branch),
        ),
        ("git_commit_sha", commit),
        ("git_remote_url_origin", origin),
        (
            "git_ref",
            variable("GITHUB_REF").or_else(|| variable("CI_COMMIT_REF_NAME")),
        ),
        (
            "ci_project_handle",
            variable("GITHUB_REPOSITORY").or_else(|| variable("CI_PROJECT_PATH")),
        ),
        ("ci_host", variable("CI_SERVER_HOST")),
    ] {
        if let Some(entry) = entry {
            value[key] = entry.into();
        }
    }
    value
}

async fn git(workspace: &Path, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command.args(args).current_dir(workspace).kill_on_drop(true);
    let output = crate::commands::util::capture_tokio_command_output(
        &mut command,
        Some(Duration::from_secs(3)),
    )
    .await
    .ok()?;
    if !output.status.success() || output.stdout_truncated || output.stdout.len() > 8192 {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}
