use std::collections::{BTreeMap, VecDeque};
use std::fs::OpenOptions;
use std::io::{BufWriter, Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fd_lock::RwLock;
use struo_cas::{ActionResult, CacheProvider, Digest};
use tokio::task::JoinSet;

use crate::directory_blob::{
    digest_directory_blob, is_directory_blob, restore_directory_blob_from_reader,
    write_directory_blob,
};
use crate::file_blob::{
    digest_file_blob, digest_raw_file, file_blob_header, restore_file_blob_from_reader,
    write_file_blob, FILE_BLOB_MAGIC,
};
use crate::{Error, OutputSymlinkMode, Result, WorkspacePath};

const RESTORE_PREFETCH_CONCURRENCY: usize = 16;
// Remote snapshots must be restored privately and checked before publication.
// Their action-digest domain is separate from clients without this envelope.
pub(crate) const REMOTE_OUTPUT_MAGIC: &[u8] = b"struo.remote-output.v1\0";

mod commit;
mod legacy_hash;
mod prefetch;
mod worker_slots;
use worker_slots::{WorkerSlot, WorkerSlots};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
mod single;

/// Materialize every cached output blob to its declared workspace path.
/// On cache hit this is what makes a downstream action see a file the
/// upstream action did not actually run on this machine.
pub(crate) async fn restore(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    restore_admitted(result, workspace_root, cache, None).await
}

pub(crate) async fn restore_admitted(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
    lease: Option<crate::ExecutionPermit>,
) -> Result<()> {
    let work = Box::pin(restore_scoped(result, workspace_root, cache, lease.clone()));
    match lease {
        Some(lease) => crate::with_execution_permit(lease, work).await,
        None => work.await,
    }
}

async fn restore_scoped(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
    lease: Option<crate::ExecutionPermit>,
) -> Result<()> {
    if result.outputs.is_empty() {
        return Ok(());
    }
    let staging = std::sync::Arc::new(StagingDir::create(workspace_root)?);
    // Classify before inspecting existing outputs: hashing a remote link on
    // the coordinator could read data that the action never declared.
    if prefetch::has_remote_outputs(result, cache, &staging).await? {
        let prefetched = prefetch::restore(result, cache, &staging).await?;
        return crate::remote::restore_output_snapshots(prefetched, workspace_root, lease).await;
    }
    let outputs = outputs_needing_restore(result, workspace_root).await?;
    if outputs.outputs.is_empty() {
        return Ok(());
    }
    let prefetched = prefetch::restore(&outputs, cache, &staging).await?;
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    let mut prefetched = prefetched;
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    if let Some(permit) = crate::resource_scope::current().filter(|_| prefetched.len() == 1) {
        return single::restore(
            prefetched.pop().expect("single prefetched output"),
            workspace_root,
            std::sync::Arc::clone(&staging),
            permit,
        )
        .await;
    }
    commit::run(|| {
        with_output_lock(workspace_root, || {
            for output in prefetched {
                let PrefetchedOutput { rel, digest, blob } = output;
                let abs = workspace_root.join(&rel);
                if existing_output_matches_digest(&abs, digest) {
                    continue;
                }
                restore_prefetched_output(&rel, &abs, blob.into_reader()?, false)?;
            }
            Ok(())
        })
    })
}

pub(crate) fn with_output_lock<T>(
    workspace_root: &Path,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let mut lock = output_restore_lock(workspace_root)?;
    let started = std::time::Instant::now();
    loop {
        match lock.try_write() {
            Ok(_guard) => return operation(),
            Err(source)
                if source.kind() == std::io::ErrorKind::WouldBlock
                    && started.elapsed() < std::time::Duration::from_secs(30) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(source) => {
                return Err(Error::RestoreOutput {
                    path: output_restore_lock_path(workspace_root)
                        .display()
                        .to_string(),
                    source,
                })
            }
        }
    }
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
    workspace_root.join(".struo/locks/output-restore.lock")
}

pub(crate) fn restore_prefetched_output(
    rel: &str,
    abs: &Path,
    mut blob: impl Read + Seek,
    remote: bool,
) -> Result<()> {
    let offset = if remote {
        let mut magic = vec![0; REMOTE_OUTPUT_MAGIC.len()];
        blob.read_exact(&mut magic)
            .map_err(|source| Error::RestoreOutput {
                path: rel.into(),
                source,
            })?;
        if magic != REMOTE_OUTPUT_MAGIC {
            return Err(Error::RestoreOutput {
                path: rel.into(),
                source: std::io::Error::other("mixed remote and local output snapshots"),
            });
        }
        REMOTE_OUTPUT_MAGIC.len() as u64
    } else {
        0
    };
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
    blob.seek(std::io::SeekFrom::Start(offset))
        .map_err(|source| Error::RestoreOutput {
            path: rel.to_string(),
            source,
        })?;
    if is_directory_blob(&prefix[..read]) {
        if remote {
            return crate::directory_blob::restore_remote_directory_blob_from_reader(
                rel, abs, blob,
            );
        }
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
    let limit = crate::resource_scope::current().map_or(RESTORE_PREFETCH_CONCURRENCY, |owner| {
        usize::try_from(owner.request().memory_bytes / (1024 * 1024))
            .unwrap_or(usize::MAX)
            .clamp(1, RESTORE_PREFETCH_CONCURRENCY)
    });
    let mut slots = WorkerSlots::new(pending.len().min(limit));
    let mut tasks = JoinSet::new();
    while let Some(slot) = slots.take() {
        let (path, digest) = pending.pop_front().expect("available validation slot");
        spawn_output_validation(&mut tasks, workspace_root, path, digest, slot);
    }
    let mut outputs = BTreeMap::new();
    while let Some(joined) = tasks.join_next().await {
        let (slot, path, digest, matches) = joined.map_err(|source| Error::RestoreOutput {
            path: "cached output validation".to_string(),
            source: std::io::Error::other(source.to_string()),
        })?;
        if !matches {
            outputs.insert(path, digest);
        }
        if let Some((path, digest)) = pending.pop_front() {
            slots.release(slot);
            spawn_output_validation(
                &mut tasks,
                workspace_root,
                path,
                digest,
                slots.take().expect("completed validation slot"),
            );
        } else {
            slots.retire(slot);
        }
    }
    for (path, digest) in &result.outputs {
        let mut candidate = path.as_str();
        while let Some((parent, _)) = candidate.rsplit_once('/') {
            if outputs.contains_key(parent) {
                outputs.insert(path.clone(), *digest);
                break;
            }
            candidate = parent;
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
    tasks: &mut JoinSet<(usize, String, Digest, bool)>,
    workspace_root: &Path,
    path: String,
    digest: Digest,
    slot: WorkerSlot,
) {
    let absolute = workspace_root.join(&path);
    tasks.spawn_blocking(crate::resource_scope::owned_blocking(move || {
        let matches =
            existing_output_matches_digest_owned(&absolute, digest, slot.owner.as_deref());
        (slot.index, path, digest, matches)
    }));
}

/// Whether the output already on disk at `path` is the one `expected` names.
pub fn existing_output_matches_digest(path: &Path, expected: Digest) -> bool {
    let owner = crate::resource_scope::current();
    existing_output_matches_digest_owned(path, expected, owner.as_deref())
}

fn existing_output_matches_digest_owned(
    path: &Path,
    expected: Digest,
    owner: Option<&crate::ResourcePermit>,
) -> bool {
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
    if !metadata.is_file() {
        return false;
    }
    if metadata.len() >= 1024 * 1024 {
        if let Some(extra) = owner.and_then(crate::ResourcePermit::try_additional_default) {
            let metadata = &metadata;
            let stop = std::sync::atomic::AtomicBool::new(false);
            let cancel = &stop;
            return std::thread::scope(|scope| {
                let raw = std::thread::Builder::new()
                    .name("struo-output-hash".to_string())
                    .spawn_scoped(scope, move || {
                        let _extra = extra;
                        legacy_hash::digest_file(path, metadata, cancel)
                            .is_ok_and(|digest| digest == expected)
                    });
                let modern =
                    digest_file_blob(path, metadata).is_ok_and(|digest| digest == expected);
                if modern {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                match raw {
                    Ok(raw) => {
                        let legacy = raw
                            .join()
                            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                        modern || legacy
                    }
                    Err(error) => {
                        tracing::debug!(%error, "output hash worker unavailable; validating serially");
                        modern
                            || digest_raw_file(path, metadata)
                                .is_ok_and(|digest| digest == expected)
                    }
                }
            });
        }
    }
    digest_file_blob(path, &metadata).is_ok_and(|digest| digest == expected)
        || digest_raw_file(path, &metadata).is_ok_and(|digest| digest == expected)
}

pub(crate) struct PrefetchedOutput {
    pub(crate) rel: String,
    pub(crate) digest: Digest,
    pub(crate) blob: struo_cas::StagedBlob,
}

struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    fn create(workspace_root: &Path) -> Result<Self> {
        let parent = workspace_root.join(".struo/tmp");
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

fn restore_legacy_file(rel: &str, abs: &Path, blob: impl Read) -> Result<()> {
    crate::restore_file::restore(abs, blob, 0o755, None).map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })
}

/// Hash and store every declared output in the CAS, returning the
/// (path -> digest) map that goes into the cached `ActionResult`.
#[cfg(test)]
pub(crate) async fn capture(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
    symlink_mode: OutputSymlinkMode,
) -> Result<BTreeMap<String, Digest>> {
    capture_admitted(outputs, workspace_root, cache, symlink_mode, None).await
}

pub(crate) async fn capture_admitted(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
    symlink_mode: OutputSymlinkMode,
    lease: Option<&crate::ExecutionPermit>,
) -> Result<BTreeMap<String, Digest>> {
    let work = Box::pin(capture_scoped(
        outputs,
        workspace_root,
        cache,
        symlink_mode,
        false,
    ));
    match lease {
        Some(lease) => crate::with_execution_permit(lease.clone(), work).await,
        None => work.await,
    }
}

async fn capture_scoped(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
    symlink_mode: OutputSymlinkMode,
    remote: bool,
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
        let digest = if let Some(guard) = crate::resource_scope::current() {
            let produce = move |mut writer: &mut dyn Write| {
                if remote {
                    writer.write_all(REMOTE_OUTPUT_MAGIC)?;
                }
                if metadata.is_dir() {
                    write_directory_blob(&abs, symlink_mode, &mut writer)
                } else {
                    write_file_blob(&abs, &metadata, &mut writer)
                }
            };
            if let Some(extra) = guard.try_additional_default() {
                cache
                    .put_produced_pipelined((guard, extra), produce)
                    .await?
            } else {
                cache.put_produced(guard, produce).await?
            }
        } else if metadata.is_dir() {
            let staging = StagingDir::create(workspace_root)?;
            let blob_path = staging.path().join("directory.blob");
            let output_path = blob_path.clone();
            let logical_path = rel.as_str().to_string();
            tokio::task::spawn_blocking({
                let abs = abs.clone();
                move || {
                    let file = std::fs::File::create(&output_path)?;
                    let mut writer = BufWriter::new(file);
                    if remote {
                        writer.write_all(REMOTE_OUTPUT_MAGIC)?;
                    }
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
            let mut header = if remote {
                REMOTE_OUTPUT_MAGIC.to_vec()
            } else {
                Vec::new()
            };
            header.extend(file_blob_header(&metadata));
            let header = Cursor::new(header);
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

pub(crate) async fn capture_remote(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<BTreeMap<String, Digest>> {
    capture_scoped(
        outputs,
        workspace_root,
        cache,
        OutputSymlinkMode::Preserve,
        true,
    )
    .await
}

#[cfg(test)]
#[path = "capture_producer_tests.rs"]
mod producer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use struo_cas::Cas;
    use tempfile::TempDir;

    fn workspace_and_cache() -> (TempDir, std::path::PathBuf, CacheProvider) {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let cas_root = tmp.path().join("cas");
        std::fs::create_dir(&workspace).unwrap();
        (tmp, workspace, CacheProvider::Local(Cas::open(cas_root)))
    }

    #[tokio::test]
    async fn owned_large_file_validation_preserves_modern_legacy_and_full_pool_fallback() {
        use crate::{with_resource_permit, ResourceLimits, ResourcePool, ResourceRequest};
        use futures::FutureExt;
        use std::sync::Arc;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("file");
        let bytes = vec![7; 2 * 1024 * 1024];
        std::fs::write(&path, &bytes).unwrap();
        let modern = digest_file_blob(&path, &std::fs::metadata(&path).unwrap()).unwrap();
        let legacy = Digest::of_bytes(&bytes);
        for cpu in [1, 2] {
            let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
                cpu,
                1024 * 1024 * 1024,
            )));
            let permit = pool.acquire(ResourceRequest::default()).await;
            with_resource_permit(permit, async {
                assert!(existing_output_matches_digest(&path, modern));
                assert!(existing_output_matches_digest(&path, legacy));
                assert!(!existing_output_matches_digest(
                    &path,
                    Digest::of_bytes(b"wrong")
                ));
            })
            .await;
            assert!(pool
                .acquire(ResourceRequest::new(cpu, 1024 * 1024 * 1024))
                .now_or_never()
                .is_some());
        }
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
    async fn restored_parent_does_not_overwrite_an_already_matching_child() {
        let (_tmp, workspace, cache) = workspace_and_cache();
        std::fs::create_dir_all(workspace.join("out/tree")).unwrap();
        std::fs::write(workspace.join("out/tree/file"), "old").unwrap();
        let mut outputs = capture(
            &[WorkspacePath::try_from("out/tree").unwrap()],
            &workspace,
            &cache,
            OutputSymlinkMode::default(),
        )
        .await
        .unwrap();
        std::fs::write(workspace.join("out/tree/file"), "new").unwrap();
        outputs.extend(
            capture(
                &[WorkspacePath::try_from("out/tree/file").unwrap()],
                &workspace,
                &cache,
                OutputSymlinkMode::default(),
            )
            .await
            .unwrap(),
        );
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        restore(&result, &workspace, &cache).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("out/tree/file")).unwrap(),
            "new"
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

        assert!(matches!(err, Error::Cas(struo_cas::Error::BlobNotFound(_))));
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
