use std::sync::Arc;

use struo_cas::ActionResult;

use super::{ReproducibilityReport, ReproducibilityTrial};
use crate::{execute, outputs, Action, Error, Executor, OutputSymlinkMode, Result, Runner};

pub(super) async fn verify(runner: &Runner, action: &Action) -> Result<ReproducibilityReport> {
    let inputs = match action {
        Action::RunCommand { inputs, .. } => inputs.clone(),
        Action::CopyPath { sources, .. } => sources.clone(),
        Action::LinkPath { source, .. } => vec![source.clone()],
        Action::WriteTreeDigest { root, .. } => vec![root.clone()],
        Action::WriteArchive { entries, .. } => entries
            .iter()
            .filter_map(|entry| entry.source.clone())
            .collect(),
        _ => Vec::new(),
    };
    let temporary = Arc::new(tempfile::tempdir().map_err(|source| Error::FileAction {
        action: "prepare_verification",
        path: runner.workspace_root().display().to_string(),
        source,
    })?);
    let workspace = temporary.path().join("workspace");
    let permit = runner
        .resources
        .acquire(Executor::Local, action.resource_request())
        .await;
    let snapshot = outputs::capture_admitted(
        &inputs,
        runner.workspace_root(),
        runner.cache(),
        OutputSymlinkMode::Preserve,
        Some(&permit),
    )
    .await?;
    drop(permit);
    let snapshot = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: snapshot,
    };
    let mut trials = Vec::new();
    for _ in 0..2 {
        let permit = runner
            .resources
            .acquire(Executor::Local, action.resource_request())
            .await
            .with_lifetime_owner(temporary.clone());
        let root = workspace.clone();
        let cwd = match action {
            Action::RunCommand { cwd: Some(cwd), .. } => cwd.resolve(&workspace),
            _ => workspace.clone(),
        };
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if root.exists() {
                std::fs::remove_dir_all(&root)?;
            }
            std::fs::create_dir_all(cwd)
        })
        .await
        .map_err(|source| Error::FileAction {
            action: "reset_verification",
            path: workspace.display().to_string(),
            source: std::io::Error::other(source),
        })?
        .map_err(|source| Error::FileAction {
            action: "reset_verification",
            path: workspace.display().to_string(),
            source,
        })?;
        let permit = runner
            .resources
            .acquire(Executor::Local, action.resource_request())
            .await
            .with_lifetime_owner(temporary.clone());
        outputs::restore_admitted(&snapshot, &workspace, runner.cache(), Some(permit)).await?;
        let permit = runner
            .resources
            .acquire(Executor::resolve(action)?, action.resource_request())
            .await
            .with_lifetime_owner(temporary.clone());
        let result = execute::run_admitted(
            action,
            &workspace,
            runner.cache(),
            false,
            false,
            None,
            Some(permit),
        )
        .await;
        trials.push(ReproducibilityTrial {
            action_digest: Some(action.digest()),
            result: result.as_ref().ok().cloned(),
            error: result.err().map(|error| error.to_string()),
            log_paths: Vec::new(),
        });
    }
    Ok(ReproducibilityReport::from_trials(trials))
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{NetworkPolicy, ResourceRequest, RunOpts, SandboxMode, WorkspacePath};

    #[tokio::test]
    async fn trials_cannot_reuse_outputs_or_overwrite_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let cache = struo_cas::CacheProvider::open_local(root.path().join("cache"));
        std::fs::write(root.path().join("out"), "original").unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "test ! -e ../out && printf fresh > ../out".into(),
            ],
            env: BTreeMap::new(),
            cwd: Some(WorkspacePath::try_from("sub").unwrap()),
            input_digest: None,
            inputs: vec![],
            outputs: vec![WorkspacePath::try_from("out").unwrap()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::Preserve,
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::Off,
            network: NetworkPolicy::Unrestricted,
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        let report = Runner::with_cache(cache, root.path(), RunOpts::default())
            .verify_reproducible(&action)
            .await
            .unwrap();
        assert!(report.passed(), "{report:?}");
        assert_eq!(report.artifacts_match, Some(true));
        assert_eq!(
            std::fs::read_to_string(root.path().join("out")).unwrap(),
            "original"
        );
    }
}
