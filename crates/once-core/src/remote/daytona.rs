use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use super::archive::{create_input_archive, install_output_archive, output_archive_file};
use super::{join_path, PreparedCommand};
use crate::stream::{self, Destination};
use crate::{
    resolve_execution_argv, resolve_execution_env, Error, RemoteExecution, Result, WorkspacePath,
};

mod cleanup;
mod client;
mod transport;

use cleanup::SandboxCleanup;
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
    let sandbox = client
        .create(
            remote.environment.as_deref(),
            command.resources,
            command.timeout_ms,
        )
        .await?;
    let cleanup = SandboxCleanup::new(client.clone(), sandbox.id.clone());

    let result = async {
        let archive = create_input_archive(workspace_root, command.inputs, "daytona").await?;
        client
            .upload(&sandbox, archive.path(), archive.len(), INPUT_ARCHIVE)
            .await?;
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

        let argv = resolve_execution_argv(command.argv, Path::new(GUEST_ROOT));
        let env = resolve_execution_env(command.env, Path::new(GUEST_ROOT));
        let response = client
            .execute(
                &sandbox,
                &command_line(&argv)?,
                Some(&workdir(command.cwd)),
                &env,
                command.timeout_ms.map(timeout_secs),
            )
            .await?;
        let result = action_result(response, cache, stream_to_parent).await?;
        if result.exit_code == 0 && !command.outputs.is_empty() {
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
            client
                .download(&sandbox, OUTPUT_ARCHIVE, archive.path())
                .await?;
            install_output_archive(archive.path(), workspace_root, command.outputs, "daytona")
                .await?;
        }
        Ok(result)
    }
    .await;

    let cleanup_result = cleanup.run().await;
    match (result, cleanup_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => {
            tracing::warn!(provider = "daytona", %cleanup_error, "failed to delete remote sandbox after execution failure");
            Err(error)
        }
    }
}

async fn action_result(
    response: ExecuteResponse,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    let exit_code = response.exit_code()?;
    let (stdout, stderr) = response.output_streams();
    stream::write_parent(&stdout, Destination::Stdout, stream_to_parent).await?;
    stream::write_parent(&stderr, Destination::Stderr, stream_to_parent).await?;
    let stdout = cache.put_blob(&stdout).await?;
    let stderr = cache.put_blob(&stderr).await?;
    Ok(ActionResult {
        exit_code,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: BTreeMap::new(),
    })
}

fn ensure_success(response: &ExecuteResponse, operation: &str) -> Result<()> {
    let exit_code = response.exit_code()?;
    if exit_code == 0 {
        return Ok(());
    }
    let (stdout, stderr) = response.output_streams();
    let detail = String::from_utf8_lossy(&stderr);
    let detail = if detail.trim().is_empty() {
        String::from_utf8_lossy(&stdout)
    } else {
        detail
    };
    Err(Error::RemoteProviderApi {
        provider: "daytona".to_string(),
        message: format!("{operation} failed with exit code {exit_code}: {detail}"),
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
    use crate::remote::test_server::{self, Response};
    use crate::ResourceRequest;

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

    #[tokio::test]
    async fn deletes_the_sandbox_after_success() {
        let (url, server) = test_server::spawn(vec![
            Response::json(r#"{"id":"daytona-test","state":"started","toolboxProxyUrl":null}"#),
            Response::empty(200),
            Response::json(r#"{"exitCode":0,"result":""}"#),
            Response::json(r#"{"exitCode":0,"result":"ok"}"#),
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
        let requests = server.await.unwrap();

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
        let requests = server.await.unwrap();

        assert!(error.to_string().contains("500"));
        assert!(requests
            .iter()
            .any(|request| request.method == "DELETE" && request.path == "/sandbox/daytona-test"));
    }
}
