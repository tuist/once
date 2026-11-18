use std::collections::BTreeMap;
use std::path::Path;

use crate::remote::archive::output_archive_file;
use crate::remote::PreparedCommand;
use crate::stream::{self, Destination};
use crate::{resolve_execution_argv, resolve_execution_env, Error, Result};
use struo_cas::{ActionResult, CacheProvider};

use super::client::{Client, Sandbox};
use super::{command_line, shell_word, timeout_secs, workdir, GUEST_ROOT};

pub(super) async fn execute(
    client: &Client,
    sandbox: &Sandbox,
    command: &PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    let stdout_file = output_archive_file(workspace_root)?;
    let stderr_file = output_archive_file(workspace_root)?;
    let output_path = |file: &tempfile::NamedTempFile| {
        format!(
            "/tmp/struo-output-{}",
            file.path()
                .file_name()
                .expect("capture file has a name")
                .to_string_lossy()
        )
    };
    let guest_stdout = output_path(&stdout_file);
    let guest_stderr = output_path(&stderr_file);
    let argv = resolve_execution_argv(command.argv, Path::new(GUEST_ROOT));
    let env = resolve_execution_env(command.env, Path::new(GUEST_ROOT));
    let redirected = format!(
        "{} > {} 2> {}",
        command_line(&argv)?,
        shell_word(&guest_stdout),
        shell_word(&guest_stderr)
    );
    let response = client
        .execute(
            sandbox,
            &redirected,
            Some(&workdir(command.cwd)),
            &env,
            command.timeout_ms.map(timeout_secs),
        )
        .await?;
    let exit_code = response.exit_code()?;
    let mut capture_client = client.clone();
    capture_client.limits.max_archive_bytes = capture_limit(client, 0);
    let stdout_phase = capture_client
        .download(
            sandbox,
            &guest_stdout,
            stdout_file.path(),
            command.lease.as_ref(),
            1,
        )
        .await?;
    let stdout_bytes = tokio::fs::metadata(stdout_file.path())
        .await
        .map_err(|source| Error::FileAction {
            action: "measure_remote_capture",
            path: ".struo/tmp".to_string(),
            source,
        })?
        .len();
    let stdout = to_cache(
        stdout_file.path(),
        Destination::Stdout,
        cache,
        stream_to_parent,
    )
    .await?;
    drop(stdout_file);
    drop(stdout_phase);
    capture_client.limits.max_archive_bytes = capture_limit(client, stdout_bytes);
    let stderr_phase = capture_client
        .download(
            sandbox,
            &guest_stderr,
            stderr_file.path(),
            command.lease.as_ref(),
            1,
        )
        .await?;
    let stderr = to_cache(
        stderr_file.path(),
        Destination::Stderr,
        cache,
        stream_to_parent,
    )
    .await?;
    drop(stderr_file);
    drop(stderr_phase);
    Ok(ActionResult {
        exit_code,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: BTreeMap::new(),
    })
}

fn capture_limit(client: &Client, consumed: u64) -> u64 {
    client
        .limits
        .max_archive_bytes
        .min(client.limits.max_capture_bytes.saturating_sub(consumed))
}

async fn to_cache(
    path: &Path,
    destination: Destination,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<struo_cas::Digest> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|source| Error::FileAction {
            action: "read_remote_capture",
            path: ".struo/tmp".to_string(),
            source,
        })?;
    stream::to_cache(file, destination, cache, stream_to_parent).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_limit_is_shared_by_stdout_and_stderr() {
        let mut client = Client::new(super::super::client::Config::for_test(
            "http://localhost".into(),
            "http://localhost".into(),
        ))
        .unwrap();
        client.limits.max_archive_bytes = 20;
        client.limits.max_capture_bytes = 10;
        assert_eq!(capture_limit(&client, 0), 10);
        assert_eq!(capture_limit(&client, 3), 7);
        assert_eq!(capture_limit(&client, 10), 0);
    }

    #[tokio::test]
    async fn file_capture_streams_past_the_protocol_response_limit() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join("cache"));
        let path = workspace.path().join("capture");
        let mut file = std::fs::File::create(&path).unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        let mut hasher = blake3::Hasher::new();
        for _ in 0..512 {
            std::io::Write::write_all(&mut file, &chunk).unwrap();
            hasher.update(&chunk);
        }
        drop(file);
        let digest = to_cache(&path, Destination::Stdout, &cache, false)
            .await
            .unwrap();
        assert_eq!(
            digest,
            struo_cas::Digest::from_bytes(*hasher.finalize().as_bytes())
        );
    }
}
