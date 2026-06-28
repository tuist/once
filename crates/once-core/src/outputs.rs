use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use once_cas::{ActionResult, CacheProvider, Digest};
use tokio::task::JoinSet;

use crate::directory_blob::{capture_directory_blob, is_directory_blob, restore_directory_blob};
use crate::file_blob::{capture_file_blob, restore_file_blob, FILE_BLOB_MAGIC};
use crate::{Error, OutputSymlinkMode, Result, WorkspacePath};

const RESTORE_PREFETCH_CONCURRENCY: usize = 16;

/// Materialize every cached output blob to its declared workspace path.
/// On cache hit this is what makes a downstream action see a file the
/// upstream action did not actually run on this machine.
pub(crate) async fn restore(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    let staging = RestoreStagingDir::create(workspace_root)?;
    let prefetched = prefetch_output_blobs(result, cache, staging.path()).await?;
    for output in prefetched {
        let PrefetchedOutput { rel, blob_path, .. } = output;
        let bytes = match tokio::fs::read(&blob_path).await {
            Ok(bytes) => bytes,
            Err(source) => return Err(Error::RestoreOutput { path: rel, source }),
        };
        let abs = workspace_root.join(&rel);
        if is_directory_blob(&bytes) {
            restore_directory_blob(&rel, &abs, &bytes)?;
            continue;
        }
        if bytes.starts_with(FILE_BLOB_MAGIC) {
            restore_file_blob(&rel, &abs, &bytes)?;
            continue;
        }
        // TODO: Remove this raw-blob compatibility branch only after an
        // action cache version bump makes old file outputs unreachable.
        restore_legacy_file(&rel, &abs, &bytes).await?;
    }
    Ok(())
}

async fn prefetch_output_blobs(
    result: &ActionResult,
    cache: &CacheProvider,
    staging_dir: &Path,
) -> Result<Vec<PrefetchedOutput>> {
    let mut outputs = result
        .outputs
        .iter()
        .enumerate()
        .map(|(index, (rel, digest))| (index, rel.clone(), *digest))
        .collect::<VecDeque<_>>();
    let output_count = outputs.len();

    let mut tasks = JoinSet::new();
    while tasks.len() < RESTORE_PREFETCH_CONCURRENCY {
        let Some((index, rel, digest)) = outputs.pop_front() else {
            break;
        };
        spawn_prefetch(&mut tasks, cache.clone(), staging_dir, index, rel, digest);
    }

    let mut blobs: Vec<Option<PrefetchedOutput>> = (0..output_count).map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let output = joined.map_err(|source| Error::RestoreOutput {
            path: "cached output prefetch".to_string(),
            source: std::io::Error::other(source.to_string()),
        })??;
        let index = output.index;
        blobs[index] = Some(output);
        if let Some((index, rel, digest)) = outputs.pop_front() {
            spawn_prefetch(&mut tasks, cache.clone(), staging_dir, index, rel, digest);
        }
    }
    blobs
        .into_iter()
        .enumerate()
        .map(|(index, output)| {
            output.ok_or_else(|| Error::RestoreOutput {
                path: format!("cached output prefetch[{index}]"),
                source: std::io::Error::other("prefetch task did not produce an output"),
            })
        })
        .collect()
}

fn spawn_prefetch(
    tasks: &mut JoinSet<Result<PrefetchedOutput>>,
    cache: CacheProvider,
    staging_dir: &Path,
    index: usize,
    rel: String,
    digest: Digest,
) {
    let blob_path = staging_dir.join(format!("{index}.blob"));
    tasks.spawn(async move {
        let bytes = cache.get_blob(&digest).await?;
        if let Err(source) = tokio::fs::write(&blob_path, bytes).await {
            return Err(Error::RestoreOutput { path: rel, source });
        }
        Ok(PrefetchedOutput {
            index,
            rel,
            blob_path,
        })
    });
}

struct PrefetchedOutput {
    index: usize,
    rel: String,
    blob_path: PathBuf,
}

struct RestoreStagingDir {
    path: PathBuf,
}

impl RestoreStagingDir {
    fn create(workspace_root: &Path) -> Result<Self> {
        let parent = workspace_root.join(".once/tmp");
        std::fs::create_dir_all(&parent).map_err(|source| Error::RestoreOutput {
            path: parent.display().to_string(),
            source,
        })?;
        for attempt in 0..100 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = parent.join(format!("restore-{}-{nanos}-{attempt}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(Error::RestoreOutput {
                        path: path.display().to_string(),
                        source,
                    });
                }
            }
        }
        Err(Error::RestoreOutput {
            path: parent.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate unique restore staging directory",
            ),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RestoreStagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

async fn restore_legacy_file(rel: &str, abs: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Error::RestoreOutput {
                path: rel.to_string(),
                source,
            })?;
    }
    let mut file = tokio::fs::File::create(&abs)
        .await
        .map_err(|source| Error::RestoreOutput {
            path: rel.to_string(),
            source,
        })?;
    file.write_all(bytes)
        .await
        .map_err(|source| Error::RestoreOutput {
            path: rel.to_string(),
            source,
        })?;
    file.flush().await.map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|source| Error::RestoreOutput {
                path: rel.to_string(),
                source,
            })?;
    }
    Ok(())
}

/// Hash and store every declared output in the CAS, returning the
/// (path -> digest) map that goes into the cached `ActionResult`.
pub(crate) async fn capture(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
    symlink_mode: OutputSymlinkMode,
) -> Result<BTreeMap<String, Digest>> {
    let mut captured = BTreeMap::new();
    for rel in outputs {
        let abs = rel.resolve(workspace_root);
        let metadata = match tokio::fs::metadata(&abs).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::MissingOutput {
                    path: rel.as_str().to_string(),
                });
            }
            Err(source) => {
                return Err(Error::ReadOutput {
                    path: rel.as_str().to_string(),
                    source,
                });
            }
        };
        let bytes = if metadata.is_dir() {
            read_output_blocking(rel.as_str(), {
                let abs = abs.clone();
                move || capture_directory_blob(&abs, symlink_mode)
            })
            .await?
        } else {
            read_output_blocking(rel.as_str(), {
                let abs = abs.clone();
                move || capture_file_blob(&abs)
            })
            .await?
        };
        let digest = cache.put_blob(&bytes).await?;
        captured.insert(rel.as_str().to_string(), digest);
    }
    Ok(captured)
}

async fn read_output_blocking(
    path: &str,
    read: impl FnOnce() -> std::io::Result<Vec<u8>> + Send + 'static,
) -> Result<Vec<u8>> {
    match tokio::task::spawn_blocking(read).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(source)) => Err(Error::ReadOutput {
            path: path.to_string(),
            source,
        }),
        Err(source) => Err(Error::ReadOutput {
            path: path.to_string(),
            source: std::io::Error::other(source.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cas::Cas;
    use tempfile::TempDir;

    fn workspace_and_cache() -> (TempDir, std::path::PathBuf, CacheProvider) {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let cas_root = tmp.path().join("cas");
        std::fs::create_dir(&workspace).unwrap();
        (tmp, workspace, CacheProvider::Local(Cas::open(cas_root)))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_outputs_restore_original_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, workspace, cache) = workspace_and_cache();
        std::fs::create_dir(workspace.join("out")).unwrap();
        let output_path = workspace.join("out/data.txt");
        std::fs::write(&output_path, b"payload").unwrap();
        std::fs::set_permissions(&output_path, std::fs::Permissions::from_mode(0o640)).unwrap();

        let output = WorkspacePath::try_from("out/data.txt").unwrap();
        let outputs = capture(
            std::slice::from_ref(&output),
            &workspace,
            &cache,
            OutputSymlinkMode::default(),
        )
        .await
        .unwrap();
        std::fs::remove_file(&output_path).unwrap();

        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        restore(&result, &workspace, &cache).await.unwrap();

        assert_eq!(std::fs::read(&output_path).unwrap(), b"payload");
        let mode = std::fs::metadata(&output_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_file_outputs_restore_with_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, workspace, cache) = workspace_and_cache();
        let digest = cache.put_blob(b"payload").await.unwrap();
        let output_path = workspace.join("out/tool");
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([("out/tool".to_string(), digest)]),
        };

        restore(&result, &workspace, &cache).await.unwrap();

        assert_eq!(std::fs::read(&output_path).unwrap(), b"payload");
        let mode = std::fs::metadata(&output_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[tokio::test]
    async fn restore_materializes_multiple_cached_outputs() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        let first = cache.put_blob(b"first").await.unwrap();
        let second = cache.put_blob(b"second").await.unwrap();
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([
                ("out/first.txt".to_string(), first),
                ("out/nested/second.txt".to_string(), second),
            ]),
        };

        restore(&result, &workspace, &cache).await.unwrap();

        assert_eq!(
            std::fs::read(workspace.join("out/first.txt")).unwrap(),
            b"first"
        );
        assert_eq!(
            std::fs::read(workspace.join("out/nested/second.txt")).unwrap(),
            b"second"
        );
    }

    #[tokio::test]
    async fn restore_materializes_outputs_beyond_prefetch_window() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        let mut outputs = BTreeMap::new();
        for index in 0..RESTORE_PREFETCH_CONCURRENCY + 3 {
            let content = format!("output-{index}");
            let digest = cache.put_blob(content.as_bytes()).await.unwrap();
            outputs.insert(format!("out/output-{index:02}.txt"), digest);
        }
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };

        restore(&result, &workspace, &cache).await.unwrap();

        for index in 0..RESTORE_PREFETCH_CONCURRENCY + 3 {
            assert_eq!(
                std::fs::read_to_string(workspace.join(format!("out/output-{index:02}.txt")))
                    .unwrap(),
                format!("output-{index}")
            );
        }
    }

    #[tokio::test]
    async fn restore_prefetches_before_materializing_outputs() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        let first = cache.put_blob(b"first").await.unwrap();
        let missing = Digest::of_bytes(b"missing");
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([
                ("out/first.txt".to_string(), first),
                ("out/missing.txt".to_string(), missing),
            ]),
        };

        let err = restore(&result, &workspace, &cache).await.unwrap_err();

        assert!(matches!(err, Error::Cas(once_cas::Error::BlobNotFound(_))));
        assert!(!workspace.join("out/first.txt").exists());
    }
}
