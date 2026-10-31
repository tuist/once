use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use super::archive::{
    create_input_archive_limited, install_output_archive_limited, output_archive_file,
};
use super::{join_path, PreparedCommand};
use crate::{Error, RemoteExecution, Result, WorkspacePath};

mod capture;
mod cleanup;
mod client;
mod transport;

use crate::ExecutionPermit;
use client::{Client, Config};
use transport::ExecuteResponse;

const GUEST_ROOT: &str = "/workspace";
const INPUT_ARCHIVE: &str = "/tmp/once-inputs.tar";
const OUTPUT_ARCHIVE: &str = "/tmp/once-outputs.tar";

pub(super) async fn execute_command(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    let config = Config::from_env()?;
    let client = Client::new(config)?;
    execute_with_client(
        remote,
        command,
        workspace_root,
        cache,
        stream_to_parent,
        client,
    )
    .await
}

async fn execute_with_client(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    client: Client,
) -> Result<ActionResult> {
    Box::pin(execute_budgeted(
        remote,
        command,
        workspace_root,
        cache,
        stream_to_parent,
        client,
    ))
    .await
}

async fn execute_budgeted(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    mut client: Client,
) -> Result<ActionResult> {
    client.limits = command
        .lease
        .as_ref()
        .map_or_else(crate::RemoteIoLimits::default, ExecutionPermit::io_limits);
    let mut deadline = super::phase::Deadline::new(command.timeout_ms);
    // The input archive is prepared before a paid machine exists.
    let (archive, input_phase) = deadline
        .within(super::phase::run(
            "daytona",
            "prepare",
            create_input_archive_limited(
                workspace_root,
                command.inputs,
                "daytona",
                client.limits,
                command.lease.as_ref(),
            ),
        ))
        .await?;
    let journal = super::lifecycle::Journal::new(workspace_root);
    // Repairing earlier runs is part of this action's bounded work; a
    // cancelled pass leaves its durable records for the next run.
    deadline
        .within(super::phase::run("daytona", "reconcile", async {
            Ok(client.reconcile(&journal).await)
        }))
        .await?;
    let (sandbox, cleanup) = deadline
        .within(super::phase::run(
            "daytona",
            "create",
            Box::pin(client.create_owned(
                Some(journal),
                remote.environment.clone(),
                command.resources.clone(),
                command.timeout_ms,
                command.lease.clone(),
            )),
        ))
        .await?;

    let result = deadline
        .within(Box::pin(run_in_sandbox(
            &client,
            sandbox,
            &command,
            workspace_root,
            cache,
            stream_to_parent,
            archive,
            input_phase,
        )))
        .await;

    // Cleanup is outside the action budget: it always runs, under its own
    // bounded requests, even when the budget is spent.
    let cleanup_result = super::phase::run("daytona", "cleanup", cleanup.run()).await;
    match (result, cleanup_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => {
            tracing::warn!(provider = "daytona", %cleanup_error, "failed to delete remote sandbox after execution failure");
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_in_sandbox(
    client: &Client,
    sandbox: client::Sandbox,
    command: &PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    archive: super::archive::TempArchive,
    input_phase: super::PhasePermit,
) -> Result<ActionResult> {
    let sandbox = super::phase::run(
        "daytona",
        "startup",
        client.wait_started(sandbox, command.timeout_ms),
    )
    .await?;
    client
        .upload(&sandbox, archive.path(), archive.len(), INPUT_ARCHIVE)
        .await?;
    drop(archive);
    drop(input_phase);
    ensure_success(
        &client
            .execute(
                &sandbox,
                &format!(
                    "mkdir -p {} && tar -xf {} -C {}",
                    shell_word(GUEST_ROOT),
                    shell_word(INPUT_ARCHIVE),
                    shell_word(GUEST_ROOT)
                ),
                None,
                &BTreeMap::new(),
                command.timeout_ms.map(timeout_secs),
            )
            .await?,
        "staging declared inputs",
    )?;

    let result = super::phase::run(
        "daytona",
        "execute",
        Box::pin(capture::execute(
            client,
            &sandbox,
            command,
            workspace_root,
            cache,
            stream_to_parent,
        )),
    )
    .await?;
    if command.accepts_exit_code(result.exit_code) && !command.outputs.is_empty() {
        ensure_success(
            &client
                .execute(
                    &sandbox,
                    &pack_outputs_command(command.outputs),
                    None,
                    &BTreeMap::new(),
                    command.timeout_ms.map(timeout_secs),
                )
                .await?,
            "packing declared outputs",
        )?;
        let archive = output_archive_file(workspace_root)?;
        let output_phase = client
            .download(
                &sandbox,
                OUTPUT_ARCHIVE,
                archive.path(),
                command.lease.as_ref(),
                2,
            )
            .await?;
        install_output_archive_limited(
            archive.path(),
            workspace_root,
            command.outputs,
            "daytona",
            client.limits,
            output_phase,
        )
        .await?;
    }
    Ok(result)
}

fn ensure_success(response: &ExecuteResponse, operation: &str) -> Result<()> {
    let exit_code = response.exit_code()?;
    if exit_code == 0 {
        return Ok(());
    }
    let (stdout, stderr) = response.output_streams();
    tracing::debug!(
        executor = "daytona",
        phase = "setup",
        exit_code,
        stdout_bytes = stdout.len(),
        stderr_bytes = stderr.len(),
        "remote setup failed"
    );
    Err(Error::RemoteProviderApi {
        provider: "daytona".to_string(),
        message: format!("{operation} failed with exit code {exit_code}"),
    })
}

fn command_line(argv: &[String]) -> Result<String> {
    if argv.is_empty() {
        return Err(Error::EmptyArgv);
    }
    Ok(argv
        .iter()
        .map(|word| shell_word(word))
        .collect::<Vec<_>>()
        .join(" "))
}

fn pack_outputs_command(outputs: &[WorkspacePath]) -> String {
    let paths = outputs
        .iter()
        .map(|path| {
            let relative = if path.as_str().is_empty() {
                ".".to_string()
            } else {
                format!("./{}", path.as_str())
            };
            shell_word(&relative)
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "tar -cf {} -C {} {paths}",
        shell_word(OUTPUT_ARCHIVE),
        shell_word(GUEST_ROOT)
    )
}

fn workdir(cwd: Option<&WorkspacePath>) -> String {
    cwd.map_or_else(
        || GUEST_ROOT.to_string(),
        |cwd| join_path(GUEST_ROOT, cwd.as_str()),
    )
}

fn timeout_secs(timeout_ms: u64) -> u64 {
    timeout_ms.div_ceil(1000).max(1)
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_quotes_argv() {
        let command = command_line(&[
            "printf".to_string(),
            "%s".to_string(),
            "hello world".to_string(),
        ])
        .unwrap();
        assert_eq!(command, "'printf' '%s' 'hello world'");
    }

    #[test]
    fn output_paths_cannot_become_options() {
        let command = pack_outputs_command(&[WorkspacePath::try_from("-danger").unwrap()]);
        assert!(command.ends_with("'./-danger'"));
    }
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod lifecycle_tests {
    use futures::poll;

    use super::super::phase::testing;
    use super::*;
    use crate::remote::test_server::{self, Response};
    use crate::{ExecutionPermit, Executor, ResourceRequest};

    #[tokio::test]
    async fn deletes_the_sandbox_after_success() {
        let (url, server) = test_server::spawn(vec![
            Response::json(r#"{"id":"daytona-test","state":"started","toolboxProxyUrl":null}"#),
            Response::empty(200),
            Response::json(r#"{"exitCode":0,"result":""}"#),
            Response::json(r#"{"exitCode":0,"result":"ok"}"#),
            Response::empty(200),
            Response::empty(200),
            Response::json(r#"{"exitCode":0,"result":""}"#),
            Response::tar_file("./reports/result.txt", b"passed\n"),
            Response::empty(204),
        ])
        .await;
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join("cache"));
        let argv = vec!["true".to_string()];
        let env = BTreeMap::new();
        let resources = ResourceRequest::default();
        let outputs = vec![WorkspacePath::try_from("reports").unwrap()];
        let command = PreparedCommand {
            argv: &argv,
            env: &env,
            cwd: None,
            inputs: &[],
            outputs: &outputs,
            resources: &resources,
            timeout_ms: None,
            success_exit_codes: &[0],
            lease: None,
        };
        let client = Client::new(Config::for_test(url.clone(), format!("{url}/toolbox"))).unwrap();

        let result = execute_with_client(
            &RemoteExecution::provider("daytona"),
            command,
            workspace.path(),
            &cache,
            false,
            client,
        )
        .await
        .unwrap();
        let requests = test_server::assert_ready(server).await.unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("reports/result.txt")).unwrap(),
            "passed\n"
        );
        assert!(requests
            .iter()
            .any(|request| request.method == "DELETE" && request.path == "/sandbox/daytona-test"));
        let create: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(create["autoDeleteInterval"], 0);
    }

    #[tokio::test]
    async fn deletes_the_sandbox_when_upload_fails() {
        let (url, server) = test_server::spawn(vec![
            Response::json(r#"{"id":"daytona-test","state":"started","toolboxProxyUrl":null}"#),
            Response::empty(500),
            Response::empty(204),
        ])
        .await;
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join("cache"));
        let argv = vec!["true".to_string()];
        let env = BTreeMap::new();
        let resources = ResourceRequest::default();
        let command = PreparedCommand {
            argv: &argv,
            env: &env,
            cwd: None,
            inputs: &[],
            outputs: &[],
            resources: &resources,
            timeout_ms: None,
            success_exit_codes: &[0],
            lease: None,
        };
        let client = Client::new(Config::for_test(url.clone(), format!("{url}/toolbox"))).unwrap();

        let error = execute_with_client(
            &RemoteExecution::provider("daytona"),
            command,
            workspace.path(),
            &cache,
            false,
            client,
        )
        .await
        .unwrap_err();
        let requests = test_server::assert_ready(server).await.unwrap();

        assert!(error.to_string().contains("500"));
        assert!(requests
            .iter()
            .any(|request| request.method == "DELETE" && request.path == "/sandbox/daytona-test"));
    }

    const STARTED: &str = r#"{"id":"daytona-test","state":"started","toolboxProxyUrl":null}"#;
    const PENDING: &str = r#"{"id":"daytona-test","state":"pending_build","toolboxProxyUrl":null}"#;

    async fn execute_true(
        url: String,
        workspace: &Path,
        lease: Option<ExecutionPermit>,
    ) -> Result<ActionResult> {
        execute_true_within(url, workspace, lease, None).await
    }

    async fn execute_true_within(
        url: String,
        workspace: &Path,
        lease: Option<ExecutionPermit>,
        timeout_ms: Option<u64>,
    ) -> Result<ActionResult> {
        let cache = CacheProvider::open_local(workspace.join("cache"));
        let argv = vec!["true".to_string()];
        let env = BTreeMap::new();
        let resources = ResourceRequest::default();
        let command = PreparedCommand {
            argv: &argv,
            env: &env,
            cwd: None,
            inputs: &[],
            outputs: &[],
            resources: &resources,
            timeout_ms,
            success_exit_codes: &[0],
            lease,
        };
        let client = Client::new(Config::for_test(url.clone(), format!("{url}/toolbox"))).unwrap();
        execute_with_client(
            &RemoteExecution::provider("daytona"),
            command,
            workspace,
            &cache,
            false,
            client,
        )
        .await
    }

    fn deleted(requests: &[test_server::Request]) -> bool {
        requests
            .iter()
            .any(|request| request.method == "DELETE" && request.path == "/sandbox/daytona-test")
    }

    async fn admit(resources: &crate::ExecutorResources) -> ExecutionPermit {
        resources
            .acquire(Executor::Daytona, &ResourceRequest::default())
            .await
    }

    #[tokio::test]
    async fn deletes_the_sandbox_when_startup_fails() {
        let (url, server) = test_server::spawn(vec![
            Response::json(PENDING),
            Response::json(
                r#"{"id":"daytona-test","state":"build_failed","toolboxProxyUrl":null}"#,
            ),
            Response::empty(204),
        ])
        .await;
        let workspace = tempfile::tempdir().unwrap();

        let error = test_server::assert_ready(execute_true(url, workspace.path(), None))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("build_failed"));
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
    }

    #[tokio::test]
    async fn lease_is_released_after_success_and_failure() {
        let success = vec![
            Response::json(STARTED),
            Response::empty(200),
            Response::json(r#"{"exitCode":0,"result":""}"#),
            Response::json(r#"{"exitCode":0,"result":""}"#),
            Response::empty(200),
            Response::empty(200),
            Response::empty(204),
        ];
        let failure = vec![
            Response::json(STARTED),
            Response::empty(500),
            Response::empty(204),
        ];
        for (responses, succeeds) in [(success, true), (failure, false)] {
            let resources = test_server::single_remote_quota();
            let lease = admit(&resources).await;
            let (url, server) = test_server::spawn(responses).await;
            let workspace = tempfile::tempdir().unwrap();

            let result =
                test_server::assert_ready(execute_true(url, workspace.path(), Some(lease))).await;

            assert_eq!(result.is_ok(), succeeds);
            assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
            test_server::assert_ready(admit(&resources)).await;
        }
    }

    #[tokio::test]
    async fn cancelled_startup_keeps_the_lease_until_deletion_finishes() {
        let resources = test_server::single_remote_quota();
        let lease = admit(&resources).await;
        let (poll, poll_hold) = Response::json(STARTED).held();
        let (delete, delete_hold) = Response::empty(204).held();
        let (url, server) = test_server::spawn(vec![Response::json(PENDING), poll, delete]).await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true(url, workspace.path(), Some(lease)));

        test_server::assert_ready(async {
            tokio::select! {
                _ = &mut execution => panic!("execution finished while startup polling was held"),
                _ = poll_hold.arrived => {}
            }
        })
        .await;
        drop(execution);
        test_server::assert_pending(admit(&resources)).await;

        poll_hold.release.send(()).unwrap();
        test_server::assert_ready(delete_hold.arrived)
            .await
            .unwrap();
        test_server::assert_pending(admit(&resources)).await;

        delete_hold.release.send(()).unwrap();
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
        test_server::assert_ready(admit(&resources)).await;
    }

    #[tokio::test]
    async fn cancelling_explicit_cleanup_keeps_the_lease_until_deletion_finishes() {
        let resources = test_server::single_remote_quota();
        let lease = admit(&resources).await;
        let (delete, delete_hold) = Response::empty(204).held();
        let (url, server) =
            test_server::spawn(vec![Response::json(STARTED), Response::empty(500), delete]).await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true(url, workspace.path(), Some(lease)));

        test_server::assert_ready(async {
            tokio::select! {
                _ = &mut execution => panic!("execution finished while its deletion was held"),
                _ = delete_hold.arrived => {}
            }
        })
        .await;
        drop(execution);
        test_server::assert_pending(admit(&resources)).await;

        delete_hold.release.send(()).unwrap();
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
        test_server::assert_ready(admit(&resources)).await;
    }

    #[tokio::test]
    async fn an_expired_budget_still_awaits_cleanup_before_returning() {
        let (upload, mut upload_hold) = Response::empty(200).held();
        let (url, server) =
            test_server::spawn(vec![Response::json(STARTED), upload, Response::empty(204)]).await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true_within(
            url,
            workspace.path(),
            None,
            Some(testing::BUDGET_MS),
        ));

        test_server::assert_ready(async {
            tokio::select! {
                _ = &mut execution => panic!("execution finished before its upload"),
                _ = &mut upload_hold.arrived => {}
            }
        })
        .await;
        testing::spend_budget().await;
        // The action abandons the upload and starts cleanup, which cannot
        // finish until the server moves past the abandoned request.
        assert!(poll!(&mut execution).is_pending());
        drop(upload_hold.release);

        let result = test_server::assert_ready(execution).await;
        assert!(matches!(result, Err(Error::Timeout(_))));
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
    }
}
