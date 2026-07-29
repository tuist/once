use std::collections::{BTreeMap, VecDeque};
use std::fs::OpenOptions;
use std::io::{BufWriter, Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fd_lock::RwLock;
use once_cas::{ActionResult, CacheProvider, Digest};
use tokio::task::JoinSet;

use crate::directory_blob::{
    digest_directory_blob, is_directory_blob, restore_directory_blob_from_reader,
    write_directory_blob,
};
use crate::file_blob::{
    digest_file_blob, file_blob_header, restore_file_blob_from_reader, FILE_BLOB_MAGIC,
};
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
    let outputs = outputs_needing_restore(result, workspace_root).await?;
    if outputs.outputs.is_empty() {
        return Ok(());
    }
    let staging = StagingDir::create(workspace_root)?;
    let prefetched = prefetch_output_blobs(&outputs, cache, staging.path()).await?;
    let mut lock = output_restore_lock(workspace_root)?;
    let _guard = lock.write().map_err(|source| Error::RestoreOutput {
        path: output_restore_lock_path(workspace_root)
            .display()
            .to_string(),
        source,
    })?;
    for output in prefetched {
        let PrefetchedOutput {
            rel,
            digest,
            blob_path,
            ..
        } = output;
        let abs = workspace_root.join(&rel);
        if existing_output_matches_digest(&abs, digest) {
            continue;
        }
        restore_prefetched_output(&rel, &abs, &blob_path)?;
    }
    Ok(())
}

fn output_restore_lock(workspace_root: &Path) -> Result<RwLock<std::fs::File>> {
    let path = output_restore_lock_path(workspace_root);
    let parent = path.parent().expect("output restore lock has a parent");
    std::fs::create_dir_all(parent).map_err(|source| Error::RestoreOutput {
        path: parent.display().to_string(),
        source,
    })?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| Error::RestoreOutput {
            path: path.display().to_string(),
            source,
        })?;
    Ok(RwLock::new(file))
}

fn output_restore_lock_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".once/locks/output-restore.lock")
}

fn restore_prefetched_output(rel: &str, abs: &Path, blob_path: &Path) -> Result<()> {
    let mut blob = std::fs::File::open(blob_path).map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    let mut prefix = [0_u8; 32];
    let mut read = 0;
    while read < prefix.len() {
        match blob.read(&mut prefix[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(source) => {
                return Err(Error::RestoreOutput {
                    path: rel.to_string(),
                    source,
                });
            }
        }
    }
    blob.rewind().map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    if is_directory_blob(&prefix[..read]) {
        return restore_directory_blob_from_reader(rel, abs, blob);
    }
    if prefix[..read].starts_with(FILE_BLOB_MAGIC) {
        return restore_file_blob_from_reader(rel, abs, blob);
    }
    restore_legacy_file(rel, abs, blob)
}

async fn outputs_needing_restore(
    result: &ActionResult,
    workspace_root: &Path,
) -> Result<ActionResult> {
    let mut pending = result
        .outputs
        .iter()
        .map(|(path, digest)| (path.clone(), *digest))
        .collect::<VecDeque<_>>();
    let mut tasks = JoinSet::new();
    while tasks.len() < RESTORE_PREFETCH_CONCURRENCY {
        let Some((path, digest)) = pending.pop_front() else {
            break;
        };
        spawn_output_validation(&mut tasks, workspace_root, path, digest);
    }
    let mut outputs = BTreeMap::new();
    while let Some(joined) = tasks.join_next().await {
        let (path, digest, matches) = joined.map_err(|source| Error::RestoreOutput {
            path: "cached output validation".to_string(),
            source: std::io::Error::other(source.to_string()),
        })?;
        if !matches {
            outputs.insert(path, digest);
        }
        if let Some((path, digest)) = pending.pop_front() {
            spawn_output_validation(&mut tasks, workspace_root, path, digest);
        }
    }
    Ok(ActionResult {
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        outputs,
    })
}

fn spawn_output_validation(
    tasks: &mut JoinSet<(String, Digest, bool)>,
    workspace_root: &Path,
    path: String,
    digest: Digest,
) {
    let absolute = workspace_root.join(&path);
    tasks.spawn_blocking(move || {
        let matches = existing_output_matches_digest(&absolute, digest);
        (path, digest, matches)
    });
}

fn existing_output_matches_digest(path: &Path, expected: Digest) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.is_dir() {
        // The capture symlink mode is not carried in the cached
        // `ActionResult`, so we cannot know which mode produced `expected`.
        // The two modes only differ for directories containing symlinks that
        // point outside the tree; for everything else they hash identically.
        // Accept a match under either mode so the "already on disk, skip
        // restore" optimization is not silently defeated for such outputs.
        // The default mode is tried first, so unchanged directories captured
        // the usual way match on the first hash.
        return [OutputSymlinkMode::default(), OutputSymlinkMode::Preserve]
            .into_iter()
            .any(|mode| digest_directory_blob(path, mode).is_ok_and(|digest| digest == expected));
    }
    if metadata.is_file()
        && digest_file_blob(path, &metadata).is_ok_and(|digest| digest == expected)
    {
        return true;
    }
    metadata.is_file()
        && std::fs::read(path).is_ok_and(|bytes| Digest::of_bytes(&bytes) == expected)
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
        cache.copy_blob_to_file(&digest, &blob_path).await?;
        Ok(PrefetchedOutput {
            index,
            rel,
            digest,
            blob_path,
        })
    });
}

struct PrefetchedOutput {
    index: usize,
    rel: String,
    digest: Digest,
    blob_path: PathBuf,
}

struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
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

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn restore_legacy_file(rel: &str, abs: &Path, mut blob: impl Read) -> Result<()> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::RestoreOutput {
            path: rel.to_string(),
            source,
        })?;
    }
    let mut file = std::fs::File::create(abs).map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    std::io::copy(&mut blob, &mut file).map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    file.flush().map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(abs, std::fs::Permissions::from_mode(0o755)).map_err(
            |source| Error::RestoreOutput {
                path: rel.to_string(),
                source,
            },
        )?;
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
        let digest = if metadata.is_dir() {
            let staging = StagingDir::create(workspace_root)?;
            let blob_path = staging.path().join("directory.blob");
            let output_path = blob_path.clone();
            let logical_path = rel.as_str().to_string();
            tokio::task::spawn_blocking({
                let abs = abs.clone();
                move || {
                    let file = std::fs::File::create(&output_path)?;
                    let mut writer = BufWriter::new(file);
                    write_directory_blob(&abs, symlink_mode, &mut writer)?;
                    writer.flush()
                }
            })
            .await
            .map_err(|source| Error::ReadOutput {
                path: logical_path.clone(),
                source: std::io::Error::other(source.to_string()),
            })?
            .map_err(|source| Error::ReadOutput {
                path: logical_path,
                source,
            })?;
            let file =
                tokio::fs::File::open(&blob_path)
                    .await
                    .map_err(|source| Error::ReadOutput {
                        path: rel.as_str().to_string(),
                        source,
                    })?;
            cache.put_stream(file).await?
        } else {
            let header = Cursor::new(file_blob_header(&metadata));
            let file = tokio::fs::File::open(&abs)
                .await
                .map_err(|source| Error::ReadOutput {
                    path: rel.as_str().to_string(),
                    source,
                })?;
            cache
                .put_stream(tokio::io::AsyncReadExt::chain(header, file))
                .await?
        };
        captured.insert(rel.as_str().to_string(), digest);
    }
    Ok(captured)
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

    #[tokio::test]
    async fn restore_does_not_rewrite_an_unchanged_file_output() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        std::fs::create_dir(workspace.join("out")).unwrap();
        let output_path = workspace.join("out/data.txt");
        std::fs::write(&output_path, b"payload").unwrap();
        let output = WorkspacePath::try_from("out/data.txt").unwrap();
        let outputs = capture(
            std::slice::from_ref(&output),
            &workspace,
            &cache,
            OutputSymlinkMode::default(),
        )
        .await
        .unwrap();
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        let modified = std::fs::metadata(&output_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        restore(&result, &workspace, &cache).await.unwrap();

        assert_eq!(
            std::fs::metadata(&output_path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[tokio::test]
    async fn restore_does_not_rewrite_an_unchanged_directory_output() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        let output_path = workspace.join("out/tree/data.txt");
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        std::fs::write(&output_path, b"payload").unwrap();
        let output = WorkspacePath::try_from("out/tree").unwrap();
        let outputs = capture(
            std::slice::from_ref(&output),
            &workspace,
            &cache,
            OutputSymlinkMode::default(),
        )
        .await
        .unwrap();
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        let modified = std::fs::metadata(&output_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        restore(&result, &workspace, &cache).await.unwrap();

        assert_eq!(
            std::fs::metadata(&output_path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_does_not_rewrite_an_unchanged_directory_output_with_external_symlink() {
        let (tmp, workspace, cache) = workspace_and_cache();
        let external = tmp.path().join("external.txt");
        std::fs::write(&external, b"external payload").unwrap();
        let tree = workspace.join("out/tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("data.txt"), b"payload").unwrap();
        std::os::unix::fs::symlink(&external, tree.join("link")).unwrap();

        let output = WorkspacePath::try_from("out/tree").unwrap();
        // Captured with the default mode, which materializes the external
        // symlink's contents into the directory blob.
        let outputs = capture(
            std::slice::from_ref(&output),
            &workspace,
            &cache,
            OutputSymlinkMode::default(),
        )
        .await
        .unwrap();
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        let modified = std::fs::metadata(&tree).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        restore(&result, &workspace, &cache).await.unwrap();

        // The on-disk tree already matches the cached digest, so restore must
        // recognize it as unchanged rather than re-materializing it.
        assert_eq!(
            std::fs::metadata(&tree).unwrap().modified().unwrap(),
            modified
        );
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

    #[tokio::test]
    async fn large_file_and_directory_outputs_roundtrip_through_streaming_paths() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        let file_path = workspace.join("out/large.bin");
        let directory_file = workspace.join("out/tree/nested.bin");
        std::fs::create_dir_all(directory_file.parent().unwrap()).unwrap();
        std::fs::File::create(&file_path)
            .unwrap()
            .set_len(32 * 1024 * 1024)
            .unwrap();
        std::fs::File::create(&directory_file)
            .unwrap()
            .set_len(16 * 1024 * 1024)
            .unwrap();
        let outputs = [
            WorkspacePath::try_from("out/large.bin").unwrap(),
            WorkspacePath::try_from("out/tree").unwrap(),
        ];

        let captured = capture(&outputs, &workspace, &cache, OutputSymlinkMode::default())
            .await
            .unwrap();
        std::fs::remove_dir_all(workspace.join("out")).unwrap();
        restore(
            &ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs: captured,
            },
            &workspace,
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::metadata(file_path).unwrap().len(),
            32 * 1024 * 1024
        );
        assert_eq!(
            std::fs::metadata(directory_file).unwrap().len(),
            16 * 1024 * 1024
        );
    }
}
