use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use super::archive::{
    create_input_archive_limited, install_output_archive_limited, output_archive_file,
};
use super::{join_path, PreparedCommand};
use crate::{
    resolve_execution_argv, resolve_execution_env, Error, RemoteExecution, Result, WorkspacePath,
};

mod cleanup;
mod client;
mod protocol;
mod transport;

use crate::executor::staging::CapturePermit;
use crate::ExecutionPermit;
use client::{Client, Config};

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
    let prepared = deadline
        .within(super::phase::run(
            "e2b",
            "prepare",
            prepare(&command, workspace_root, client.limits),
        ))
        .await?;
    let journal = super::lifecycle::Journal::new(workspace_root);
    // Repairing earlier runs is part of this action's bounded work; a
    // cancelled pass leaves its durable records for the next run.
    deadline
        .within(super::phase::run("e2b", "reconcile", async {
            Ok(client.reconcile(&journal).await)
        }))
        .await?;
    let (sandbox, cleanup) = deadline
        .within(super::phase::run(
            "e2b",
            "create",
            Box::pin(client.create_owned(
                Some(journal),
                remote.environment.clone(),
                command.timeout_ms,
                command.lease.clone(),
            )),
        ))
        .await?;
    let result = deadline
        .within(Box::pin(run_in_sandbox(
            &client,
            &sandbox,
            &command,
            workspace_root,
            cache,
            stream_to_parent,
            prepared,
        )))
        .await;

    // Cleanup is outside the action budget: it always runs, under its own
    // bounded requests, even when the budget is spent.
    let cleanup_result = super::phase::run("e2b", "cleanup", cleanup.run()).await;
    match (result, cleanup_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => {
            tracing::warn!(provider = "e2b", %cleanup_error, "failed to delete remote sandbox after execution failure");
            Err(error)
        }
    }
}

/// Work that can wait or fail for local reasons, done before a paid machine
/// exists: the input archive and the action's whole capture allowance.
struct Prepared {
    archive: super::archive::TempArchive,
    input_phase: super::PhasePermit,
    capture: Option<CapturePermit>,
}

async fn prepare(
    command: &PreparedCommand<'_>,
    workspace_root: &Path,
    limits: crate::RemoteIoLimits,
) -> Result<Prepared> {
    let (archive, input_phase) = create_input_archive_limited(
        workspace_root,
        command.inputs,
        "e2b",
        limits,
        command.lease.as_ref(),
    )
    .await?;
    let capture = match command.lease.as_ref() {
        Some(lease) => {
            let capture = lease.capture_budget();
            capture.reserve_upfront().await?;
            Some(capture)
        }
        None => None,
    };
    Ok(Prepared {
        archive,
        input_phase,
        capture,
    })
}

async fn run_in_sandbox(
    client: &Client,
    sandbox: &client::Sandbox,
    command: &PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    prepared: Prepared,
) -> Result<ActionResult> {
    let Prepared {
        archive,
        input_phase,
        capture,
    } = prepared;
    client
        .upload(sandbox, archive.path(), archive.len(), INPUT_ARCHIVE)
        .await?;
    drop(archive);
    drop(input_phase);
    run_setup(
        client,
        sandbox,
        "mkdir",
        &["-p".to_string(), GUEST_ROOT.to_string()],
        command.timeout_ms,
    )
    .await?;
    run_setup(
        client,
        sandbox,
        "tar",
        &[
            "-xf".to_string(),
            INPUT_ARCHIVE.to_string(),
            "-C".to_string(),
            GUEST_ROOT.to_string(),
        ],
        command.timeout_ms,
    )
    .await?;

    let resolved_command = resolve_execution_argv(command.argv, Path::new(GUEST_ROOT));
    let env = resolve_execution_env(command.env, Path::new(GUEST_ROOT));
    let (program, command_args) = resolved_command.split_first().ok_or(Error::EmptyArgv)?;
    let result = super::phase::run(
        "e2b",
        "execute",
        client.execute_streaming(
            sandbox,
            program,
            command_args,
            Some(&workdir(command.cwd)),
            &env,
            command.timeout_ms,
            cache,
            stream_to_parent,
            capture,
        ),
    )
    .await?;
    if command.accepts_exit_code(result.exit_code) && !command.outputs.is_empty() {
        let output_args = pack_output_args(command.outputs);
        run_setup(client, sandbox, "tar", &output_args, command.timeout_ms).await?;
        let archive = output_archive_file(workspace_root)?;
        let output_phase = client
            .download(
                sandbox,
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
            "e2b",
            client.limits,
            output_phase,
        )
        .await?;
    }
    Ok(result)
}

async fn run_setup(
    client: &Client,
    sandbox: &client::Sandbox,
    program: &str,
    args: &[String],
    timeout_ms: Option<u64>,
) -> Result<()> {
    let response = client
        .execute(sandbox, program, args, None, &BTreeMap::new(), timeout_ms)
        .await?;
    tracing::debug!(
        executor = "e2b",
        phase = "setup",
        exit_code = response.exit_code,
        stdout_bytes = response.stdout.len(),
        stderr_bytes = response.stderr.len(),
        "remote setup completed"
    );
    if response.exit_code == 0 {
        Ok(())
    } else {
        Err(Error::RemoteProviderApi {
            provider: "e2b".to_string(),
            message: format!("remote setup failed with exit code {}", response.exit_code),
        })
    }
}

fn pack_output_args(outputs: &[WorkspacePath]) -> Vec<String> {
    let mut args = vec![
        "-cf".to_string(),
        OUTPUT_ARCHIVE.to_string(),
        "-C".to_string(),
        GUEST_ROOT.to_string(),
    ];
    args.extend(outputs.iter().map(|path| {
        if path.as_str().is_empty() {
            ".".to_string()
        } else {
            format!("./{}", path.as_str())
        }
    }));
    args
}

fn workdir(cwd: Option<&WorkspacePath>) -> String {
    cwd.map_or_else(
        || GUEST_ROOT.to_string(),
        |cwd| join_path(GUEST_ROOT, cwd.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_paths_cannot_become_options() {
        let args = pack_output_args(&[WorkspacePath::try_from("-danger").unwrap()]);
        assert_eq!(args.last().unwrap(), "./-danger");
    }
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod lifecycle_tests {
    use std::future::Future;
    use std::task::Poll;

    use futures::poll;

    use super::super::phase::testing;
    use super::*;
    use crate::remote::test_server::{self, Response};
    use crate::{ExecutionPermit, Executor, ResourceRequest};

    #[tokio::test]
    async fn deletes_the_sandbox_after_success() {
        let process = process_response(0);
        let (url, server) = test_server::spawn(vec![
            Response::json(
                r#"{"sandboxID":"e2b-test","envdAccessToken":"access-token","envdVersion":"1.0.0"}"#,
            ),
            Response::empty(200),
            process_response_spec(&process),
            process_response_spec(&process),
            process_response_spec(&process),
            process_response_spec(&process),
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
        let client = Client::new(Config::for_test(url.clone(), url)).unwrap();

        let result = execute_with_client(
            &RemoteExecution::provider("e2b"),
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
        assert!(requests.iter().any(|request| {
            request.method == "DELETE" && request.path == "/sandboxes/e2b-test"
        }));
        let create: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(create["autoPause"], false);
        assert_eq!(create["autoResume"]["enabled"], false);
    }

    #[tokio::test]
    async fn deletes_the_sandbox_when_upload_fails() {
        let (url, server) = test_server::spawn(vec![
            Response::json(
                r#"{"sandboxID":"e2b-test","envdAccessToken":"access-token","envdVersion":"1.0.0"}"#,
            ),
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
        let client = Client::new(Config::for_test(url.clone(), url)).unwrap();

        let error = execute_with_client(
            &RemoteExecution::provider("e2b"),
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
        assert!(requests.iter().any(|request| {
            request.method == "DELETE" && request.path == "/sandboxes/e2b-test"
        }));
    }

    const CREATED: &str =
        r#"{"sandboxID":"e2b-test","envdAccessToken":"access-token","envdVersion":"1.0.0"}"#;

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
        let client = Client::new(Config::for_test(url.clone(), url)).unwrap();
        execute_with_client(
            &RemoteExecution::provider("e2b"),
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
            .any(|request| request.method == "DELETE" && request.path == "/sandboxes/e2b-test")
    }

    async fn admit(resources: &crate::ExecutorResources) -> ExecutionPermit {
        resources
            .acquire(Executor::E2b, &ResourceRequest::default())
            .await
    }

    #[tokio::test]
    async fn lease_is_released_after_success_and_failure() {
        let success = vec![
            Response::json(CREATED),
            Response::empty(200),
            process_response_spec(&process_response(0)),
            process_response_spec(&process_response(0)),
            process_response_spec(&process_response(0)),
            Response::empty(204),
        ];
        let failure = vec![
            Response::json(CREATED),
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
    async fn cancelled_execution_keeps_the_lease_until_deletion_finishes() {
        let resources = test_server::single_remote_quota();
        let lease = admit(&resources).await;
        let (upload, upload_hold) = Response::empty(200).held();
        let (delete, delete_hold) = Response::empty(204).held();
        let (url, server) = test_server::spawn(vec![Response::json(CREATED), upload, delete]).await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true(url, workspace.path(), Some(lease)));

        test_server::assert_ready(async {
            tokio::select! {
                _ = &mut execution => panic!("execution finished while its upload was held"),
                _ = upload_hold.arrived => {}
            }
        })
        .await;
        drop(execution);
        test_server::assert_pending(admit(&resources)).await;

        upload_hold.release.send(()).unwrap();
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
            test_server::spawn(vec![Response::json(CREATED), Response::empty(500), delete]).await;
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
    async fn a_full_capture_pool_holds_the_action_before_any_sandbox_is_created() {
        let resources = test_server::single_remote_quota();
        let lease = admit(&resources).await;
        // Other actions hold the whole capture budget.
        let mut occupied = Vec::new();
        loop {
            let capture = lease.capture_budget();
            match tokio::time::timeout(
                std::time::Duration::from_millis(20),
                capture.reserve_upfront(),
            )
            .await
            {
                Ok(reserved) => {
                    reserved.unwrap();
                    occupied.push(capture);
                }
                Err(_) => break,
            }
        }
        assert!(!occupied.is_empty());
        let (create, mut create_hold) = Response::json(CREATED).held();
        let (url, server) = test_server::spawn(vec![
            create,
            Response::empty(200),
            process_response_spec(&process_response(0)),
            process_response_spec(&process_response(0)),
            process_response_spec(&process_response(0)),
            Response::empty(204),
        ])
        .await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true(url, workspace.path(), Some(lease)));

        test_server::assert_pending(&mut execution).await;
        assert!(
            create_hold.arrived.try_recv().is_err(),
            "a sandbox was requested before capture capacity was reserved"
        );

        occupied.pop();
        let (result, ()) = test_server::assert_ready(async {
            tokio::join!(execution, async {
                (&mut create_hold.arrived).await.unwrap();
                create_hold.release.send(()).unwrap();
            })
        })
        .await;
        assert_eq!(result.unwrap().exit_code, 0);
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
    }

    /// Wait until `hold` sees its request, failing if `execution` ends first.
    async fn reach<F: Future + Unpin>(execution: &mut F, hold: &mut test_server::Hold) {
        test_server::assert_ready(async {
            tokio::select! {
                _ = execution => panic!("execution finished before the held request"),
                _ = &mut hold.arrived => {}
            }
        })
        .await;
    }

    #[tokio::test]
    async fn a_slow_cleanup_never_turns_a_finished_action_into_a_timeout() {
        let (delete, mut delete_hold) = Response::empty(204).held();
        let (url, server) = test_server::spawn(vec![
            Response::json(CREATED),
            Response::empty(200),
            process_response_spec(&process_response(0)),
            process_response_spec(&process_response(0)),
            process_response_spec(&process_response(0)),
            delete,
        ])
        .await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true_within(
            url,
            workspace.path(),
            None,
            Some(testing::BUDGET_MS),
        ));

        reach(&mut execution, &mut delete_hold).await;
        // The work is done; cleanup now outlasts the whole budget.
        testing::spend_budget().await;
        assert!(poll!(&mut execution).is_pending());
        delete_hold.release.send(()).unwrap();

        let result = test_server::assert_ready(execution).await;
        assert_eq!(result.unwrap().exit_code, 0);
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
    }

    #[tokio::test]
    async fn an_expired_budget_still_awaits_cleanup_before_returning() {
        let (upload, mut upload_hold) = Response::empty(200).held();
        let (url, server) =
            test_server::spawn(vec![Response::json(CREATED), upload, Response::empty(204)]).await;
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = Box::pin(execute_true_within(
            url,
            workspace.path(),
            None,
            Some(testing::BUDGET_MS),
        ));

        reach(&mut execution, &mut upload_hold).await;
        testing::spend_budget().await;
        // The action abandons the upload and starts cleanup, which cannot
        // finish until the server moves past the abandoned request.
        assert!(poll!(&mut execution).is_pending());
        drop(upload_hold.release);

        let result = test_server::assert_ready(execution).await;
        assert!(matches!(result, Err(Error::Timeout(_))));
        assert!(deleted(&test_server::assert_ready(server).await.unwrap()));
    }

    #[tokio::test]
    async fn reconciling_earlier_runs_counts_against_the_action_budget() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = super::super::lifecycle::Journal::new(workspace.path());
        super::super::lifecycle::tests::intent(&journal);
        let (listing, mut listing_hold) = Response::json("[]").held();
        let (url, server) = test_server::spawn(vec![listing]).await;
        let probe = Client::new(Config::for_test(url.clone(), url.clone())).unwrap();
        super::super::lifecycle::tests::rescope(
            workspace.path(),
            "e2b",
            &probe.scope(),
            &probe.credential(),
        );
        let mut execution = Box::pin(execute_true_within(
            url,
            workspace.path(),
            None,
            Some(testing::BUDGET_MS),
        ));

        reach(&mut execution, &mut listing_hold).await;
        testing::spend_budget().await;
        let first = poll!(&mut execution);
        drop(listing_hold.release);
        let result = match first {
            Poll::Ready(result) => result,
            Poll::Pending => test_server::assert_ready(execution).await,
        };

        assert!(matches!(result, Err(Error::Timeout(_))));
        let requests = test_server::assert_ready(server).await.unwrap();
        assert!(
            requests.iter().all(|request| request.method == "GET"),
            "a sandbox was created after the budget was spent"
        );
    }

    fn process_response(exit_code: i32) -> Vec<u8> {
        let payload = serde_json::to_vec(
            &serde_json::json!({"event":{"end":{"exitCode":exit_code,"exited":true}}}),
        )
        .unwrap();
        let mut result = vec![0];
        result.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
        result.extend(payload);
        let end = b"{}";
        result.push(2);
        result.extend(u32::try_from(end.len()).unwrap().to_be_bytes());
        result.extend(end);
        result
    }

    fn process_response_spec(body: &[u8]) -> Response {
        Response {
            status: 200,
            content_type: "application/connect+json",
            body: body.to_vec(),
            hold: None,
        }
    }
}
