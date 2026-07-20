use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use super::archive::{create_input_archive, install_output_archive, output_archive_file};
use super::{join_path, PreparedCommand};
use crate::stream::{self, Destination};
use crate::{Error, RemoteExecution, Result, WorkspacePath};

mod cleanup;
mod client;
mod protocol;
mod transport;

use cleanup::SandboxCleanup;
use client::{Client, Config};
use protocol::CommandResponse;

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
    let sandbox = client
        .create(remote.environment.as_deref(), command.timeout_ms)
        .await?;
    let cleanup = SandboxCleanup::new(client.clone(), sandbox.id.clone());

    let result = async {
        let archive = create_input_archive(workspace_root, command.inputs, "e2b").await?;
        client
            .upload(&sandbox, archive.path(), archive.len(), INPUT_ARCHIVE)
            .await?;
        run_setup(
            &client,
            &sandbox,
            "mkdir",
            &["-p".to_string(), GUEST_ROOT.to_string()],
            command.timeout_ms,
        )
        .await?;
        run_setup(
            &client,
            &sandbox,
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

        let (program, args) = command.argv.split_first().ok_or(Error::EmptyArgv)?;
        let response = client
            .execute(
                &sandbox,
                program,
                args,
                Some(&workdir(command.cwd)),
                command.env,
                command.timeout_ms,
            )
            .await?;
        let result = action_result(response, cache, stream_to_parent).await?;
        if result.exit_code == 0 && !command.outputs.is_empty() {
            let args = pack_output_args(command.outputs);
            run_setup(&client, &sandbox, "tar", &args, command.timeout_ms).await?;
            let archive = output_archive_file(workspace_root)?;
            client
                .download(&sandbox, OUTPUT_ARCHIVE, archive.path())
                .await?;
            install_output_archive(archive.path(), workspace_root, command.outputs, "e2b").await?;
        }
        Ok(result)
    }
    .await;

    let cleanup_result = cleanup.run().await;
    match (result, cleanup_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => {
            tracing::warn!(provider = "e2b", %cleanup_error, "failed to delete remote sandbox after execution failure");
            Err(error)
        }
    }
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
    if response.exit_code == 0 {
        Ok(())
    } else {
        Err(Error::RemoteProviderApi {
            provider: "e2b".to_string(),
            message: format!(
                "remote setup failed with exit code {}: {}",
                response.exit_code,
                String::from_utf8_lossy(&response.stderr)
            ),
        })
    }
}

async fn action_result(
    response: CommandResponse,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    stream::write_parent(&response.stdout, Destination::Stdout, stream_to_parent).await?;
    stream::write_parent(&response.stderr, Destination::Stderr, stream_to_parent).await?;
    let stdout = cache.put_blob(&response.stdout).await?;
    let stderr = cache.put_blob(&response.stderr).await?;
    Ok(ActionResult {
        exit_code: response.exit_code,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: BTreeMap::new(),
    })
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
    use crate::remote::test_server::{self, Response};
    use crate::ResourceRequest;

    #[test]
    fn output_paths_cannot_become_options() {
        let args = pack_output_args(&[WorkspacePath::try_from("-danger").unwrap()]);
        assert_eq!(args.last().unwrap(), "./-danger");
    }

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
        let requests = server.await.unwrap();

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
        let requests = server.await.unwrap();

        assert!(error.to_string().contains("500"));
        assert!(requests.iter().any(|request| {
            request.method == "DELETE" && request.path == "/sandboxes/e2b-test"
        }));
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
        }
    }
}
