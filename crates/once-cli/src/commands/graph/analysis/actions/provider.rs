use std::io::Read as _;
use std::path::Path;

use anyhow::Result;
use once_cas::{ActionResult, CacheProvider};
use once_core::{Action, ActionOutputObserver, ActionOutputStream, ExecutionPermit};

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
    observer: Option<&dyn ActionOutputObserver>,
    lease: ExecutionPermit,
    clean_paths: &[String],
    create_dirs: &[String],
    arg_files: &[once_frontend::analysis::DeclaredArgFile],
) -> Result<ActionResult> {
    let result = if once_core::Executor::resolve(action)? != once_core::Executor::Local
        && matches!(action, Action::RunCommand { .. })
    {
        let clean_paths = clean_paths
            .iter()
            .map(|path| super::workspace_path(path, "clean_paths entry"))
            .collect::<Result<Vec<_>>>()?;
        let create_dirs = create_dirs
            .iter()
            .map(|path| super::workspace_path(path, "create_dirs entry"))
            .collect::<Result<Vec<_>>>()?;
        let post_setup_inputs = arg_files
            .iter()
            .map(|file| super::workspace_path(&file.path, "arg_files path"))
            .collect::<Result<Vec<_>>>()?;
        once_core::Runner::run_uncached_admitted_with_setup(
            action,
            workspace,
            cache,
            false,
            None,
            lease,
            &clean_paths,
            &create_dirs,
            &post_setup_inputs,
        )
        .await
    } else {
        once_core::run_uncached_admitted(action, workspace, cache, false, observer, lease).await
    };
    match result {
        Ok(result) => Ok(result),
        Err(error) => Err(super::render_execution_error(error, cache).await),
    }
}

pub(super) async fn replay_output(
    result: &ActionResult,
    cache: &CacheProvider,
    observer: Option<&dyn ActionOutputObserver>,
    workspace: &Path,
    lease: &ExecutionPermit,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };
    let parent = workspace.join(".once/tmp");
    tokio::fs::create_dir_all(&parent).await?;
    for (digest, stream) in [
        (result.stdout, ActionOutputStream::Stdout),
        (result.stderr, ActionOutputStream::Stderr),
    ] {
        let Some(digest) = digest else { continue };
        let mut reader = cache
            .stage_blob_guarded(&digest, &parent, lease.clone())
            .await?
            .into_reader()?;
        loop {
            let (next, bytes) = lease
                .run_blocking(move || {
                    let mut bytes = vec![0; 64 * 1024];
                    let count = reader.read(&mut bytes)?;
                    bytes.truncate(count);
                    Ok::<_, std::io::Error>((reader, bytes))
                })
                .await??;
            reader = next;
            if bytes.is_empty() {
                break;
            }
            observer.observe(stream, &bytes);
        }
    }
    Ok(())
}
