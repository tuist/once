use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use once_cas::{ActionResult, CacheProvider};
use sha2::Digest as ShaDigest;
use std::io::Write as _;

mod sandbox;
mod zip_preflight;

#[cfg(all(test, unix))]
mod admission_tests;
#[cfg(test)]
mod download_tests;

use crate::executor::staging::StagingPermit;
use crate::stream::ActionOutputObserver;
use crate::{
    archive, local, outputs, remote, Action, CopyPathMode, Error, ExecutionPermit, Executor,
    OutputSymlinkMode, PreparePathMode, RemoteIoLimits, Result, SandboxMode, WorkspacePath,
};

pub(crate) async fn run(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
) -> Result<ActionResult> {
    run_observed(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        validate_contract,
        None,
    )
    .await
}

pub(crate) async fn run_observed(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
    observer: Option<&dyn ActionOutputObserver>,
) -> Result<ActionResult> {
    run_admitted(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        validate_contract,
        observer,
        None,
    )
    .await
}

pub(crate) async fn run_admitted(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
    observer: Option<&dyn ActionOutputObserver>,
    lease: Option<ExecutionPermit>,
) -> Result<ActionResult> {
    run_admitted_with_setup(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        validate_contract,
        observer,
        lease,
        remote::CommandFilesystemSetup::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_admitted_with_setup(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
    observer: Option<&dyn ActionOutputObserver>,
    lease: Option<ExecutionPermit>,
    setup: remote::CommandFilesystemSetup<'_>,
) -> Result<ActionResult> {
    let work = Box::pin(run_scoped(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        validate_contract,
        observer,
        lease.clone(),
        setup,
    ));
    match lease {
        Some(lease) => crate::with_execution_permit(lease, work).await,
        None => work.await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_scoped(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
    observer: Option<&dyn ActionOutputObserver>,
    lease: Option<ExecutionPermit>,
    setup: remote::CommandFilesystemSetup<'_>,
) -> Result<ActionResult> {
    if let Action::RunCommand {
        outputs,
        output_symlink_mode,
        ..
    } = action
    {
        let (mut result, outputs_captured) = Box::pin(execute_command(
            action,
            workspace_root,
            cache,
            stream_to_parent,
            validate_contract,
            observer,
            lease.clone(),
            setup,
        ))
        .await?;
        action.normalize_result(&mut result)?;
        let completed = result.exit_code == 0;
        if completed && !validate_contract && !outputs_captured {
            result.outputs = outputs::capture_admitted(
                outputs,
                workspace_root,
                cache,
                *output_symlink_mode,
                lease.as_ref(),
            )
            .await?;
        }
        return Ok(result);
    }

    execute_portable_action(action, workspace_root, cache, lease.as_ref()).await
}

/// Start filesystem work on the blocking pool, owning a clone of the
/// admission lease. A blocking task keeps running after its caller is
/// cancelled, so the task, not the awaiting future, holds the reservation
/// until the work actually stops.
pub(crate) fn spawn_admitted<T, F>(
    lease: Option<&ExecutionPermit>,
    work: F,
) -> tokio::task::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let lease = lease.cloned();
    tokio::task::spawn_blocking(crate::resource_scope::owned_blocking(move || {
        let _lease = lease;
        work()
    }))
}

/// [`spawn_admitted`] for one I/O step, reported as a failed `action` on
/// `path`.
async fn file_step<T, F>(
    lease: Option<&ExecutionPermit>,
    action: &'static str,
    path: &str,
    work: F,
) -> Result<T>
where
    F: FnOnce() -> std::io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    spawn_admitted(lease, work)
        .await
        .unwrap_or_else(|join| Err(std::io::Error::other(join.to_string())))
        .map_err(|source| Error::FileAction {
            action,
            path: path.to_string(),
            source,
        })
}

#[allow(clippy::too_many_lines)]
async fn execute_portable_action(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    lease: Option<&ExecutionPermit>,
) -> Result<ActionResult> {
    match action {
        Action::WriteFile { path, bytes, .. } => {
            write_file(path, bytes, workspace_root, lease).await?;
            capture_file_action_outputs(std::slice::from_ref(path), workspace_root, cache, lease)
                .await
        }
        Action::CopyPath {
            sources,
            destination,
            mode,
            ..
        } => {
            match mode {
                CopyPathMode::File => {
                    if sources.len() != 1 {
                        return Err(Error::InvalidCopyPath {
                            reason: "file copy requires exactly one source".to_string(),
                        });
                    }
                    copy_file(&sources[0], destination, workspace_root, lease).await?;
                }
                CopyPathMode::Tree => {
                    copy_tree(sources, destination, workspace_root, lease).await?;
                }
            }
            capture_file_action_outputs(
                std::slice::from_ref(destination),
                workspace_root,
                cache,
                lease,
            )
            .await
        }
        Action::LinkPath {
            source,
            destination,
            ..
        } => {
            link_path(source, destination, workspace_root, lease).await?;
            Ok(empty_file_action_result())
        }
        Action::MaterializeHostFile {
            source,
            source_sha256,
            destination,
            ..
        } => {
            materialize_host_file(source, source_sha256, destination, workspace_root, lease)
                .await?;
            capture_file_action_outputs(
                std::slice::from_ref(destination),
                workspace_root,
                cache,
                lease,
            )
            .await
        }
        Action::MaterializeHostTree {
            source,
            source_sha256,
            destination,
            ..
        } => {
            materialize_host_tree(source, source_sha256, destination, workspace_root, lease)
                .await?;
            capture_file_action_outputs(
                std::slice::from_ref(destination),
                workspace_root,
                cache,
                lease,
            )
            .await
        }
        Action::PreparePath { path, mode, .. } => match mode {
            PreparePathMode::Remove => {
                remove_path(path, workspace_root, lease).await?;
                Ok(empty_file_action_result())
            }
            PreparePathMode::Directory => {
                ensure_dir(path, workspace_root, lease).await?;
                capture_file_action_outputs(
                    std::slice::from_ref(path),
                    workspace_root,
                    cache,
                    lease,
                )
                .await
            }
        },
        Action::WriteTreeDigest {
            root,
            output,
            include_suffixes,
            ..
        } => {
            write_tree_digest(root, output, include_suffixes, workspace_root, lease).await?;
            capture_file_action_outputs(std::slice::from_ref(output), workspace_root, cache, lease)
                .await
        }
        Action::WriteArchive {
            entries,
            output,
            sha256_output,
            uncompressed_sha256_output,
            format,
            ..
        } => {
            archive::write(
                entries,
                output,
                sha256_output.as_ref(),
                uncompressed_sha256_output.as_ref(),
                *format,
                workspace_root,
                lease,
            )
            .await?;
            let mut outputs = vec![output.clone()];
            for path in [sha256_output, uncompressed_sha256_output]
                .into_iter()
                .flatten()
            {
                outputs.push(path.clone());
            }
            capture_file_action_outputs(&outputs, workspace_root, cache, lease).await
        }
        Action::DownloadAndExtract {
            url,
            sha256,
            destination,
            authorization_env,
            ..
        } => {
            download_and_extract(
                url,
                sha256,
                destination,
                authorization_env.as_deref(),
                workspace_root,
                lease,
            )
            .await?;
            capture_file_action_outputs(
                std::slice::from_ref(destination),
                workspace_root,
                cache,
                lease,
            )
            .await
        }
        Action::RunCommand { .. } => unreachable!("command actions are dispatched separately"),
    }
}

#[allow(clippy::too_many_lines)]
async fn download_and_extract(
    url: &str,
    expected_sha256: &str,
    destination: &WorkspacePath,
    authorization_env: Option<&str>,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|source| Error::InvalidDownloadAndExtract {
        reason: format!("invalid URL: {source}"),
    })?;
    let display_url = redacted_url(&parsed);
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::InvalidDownloadAndExtract {
            reason: "URL must use HTTP or HTTPS".to_string(),
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::InvalidDownloadAndExtract {
            reason: "URL user-info is not allowed; use authorization_env instead".to_string(),
        });
    }
    if !is_sha256(expected_sha256) {
        return Err(Error::InvalidDownloadAndExtract {
            reason: "sha256 must be exactly 64 hexadecimal characters".to_string(),
        });
    }
    let absolute_destination = destination.resolve(workspace_root);
    let destination_parent =
        absolute_destination
            .parent()
            .ok_or_else(|| Error::InvalidDownloadAndExtract {
                reason: format!(
                    "destination `{}` has no parent directory",
                    destination.as_str()
                ),
            })?;
    let parent = destination_parent.to_path_buf();
    file_step(
        lease,
        "download_and_extract",
        destination.as_str(),
        move || std::fs::create_dir_all(parent),
    )
    .await?;

    let client =
        reqwest::Client::builder()
            .build()
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("creating HTTP client: {source}"),
            })?;
    let mut request = client.get(parsed);
    if let Some(name) = authorization_env {
        let value = std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::DownloadAuthorizationMissing {
                name: name.to_string(),
            })?;
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let limits = lease.map_or_else(RemoteIoLimits::default, ExecutionPermit::io_limits);
    // One absolute deadline covers the response headers, the whole body, and
    // its writes to the private archive, so a server that keeps trickling
    // bytes cannot hold the action forever. Running out of time drops only
    // this future: a write already on the blocking pool keeps its own handle
    // on the archive, its staging, and the lease until it stops. Extraction
    // starts after the deadline and is not covered by it.
    let deadline = tokio::time::Instant::now() + limits.artifact_download_timeout;
    let download = async {
        let response = tokio::time::timeout(limits.stall_timeout, request.send())
            .await
            .map_err(|_| download_stalled(limits))?
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("requesting `{display_url}`: {}", source.without_url()),
            })?
            .error_for_status()
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("requesting `{display_url}`: {}", source.without_url()),
            })?;
        let declared = response.content_length();
        check_download_size(declared.unwrap_or(0), limits)?;
        let staging = match lease {
            Some(lease) => Some(
                lease
                    .acquire_staging(download_staging_bytes(declared, limits))
                    .await?,
            ),
            None => None,
        };

        let archive_parent = destination_parent.to_path_buf();
        // Created on the blocking pool with its staging, so the two are
        // never apart.
        let archive = Arc::new(
            file_step(
                lease,
                "download_and_extract",
                destination.as_str(),
                move || {
                    Ok(DownloadedArchive {
                        file: tempfile::NamedTempFile::new_in(archive_parent)?,
                        _staging: staging,
                    })
                },
            )
            .await?,
        );
        let archive_label = archive.file.path().display().to_string();
        let mut digest = sha2::Sha256::new();
        let mut stream = response.bytes_stream();
        let mut size = 0_u64;
        while let Some(chunk) = tokio::time::timeout(limits.stall_timeout, stream.next())
            .await
            .map_err(|_| download_stalled(limits))?
        {
            let chunk = chunk.map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("reading `{display_url}`: {}", source.without_url()),
            })?;
            size = size.saturating_add(chunk.len() as u64);
            check_download_size(size, limits)?;
            // The staging reservation covers the declared length only.
            if declared.is_some_and(|declared| size > declared) {
                return Err(Error::InvalidDownloadAndExtract {
                    reason: format!("`{display_url}` sent more bytes than it declared"),
                });
            }
            digest.update(&chunk);
            write_archive_chunk(lease, Arc::clone(&archive), &archive_label, chunk).await?;
        }
        Ok::<_, Error>((archive, digest, declared))
    };
    let (archive, digest, declared) = tokio::time::timeout_at(deadline, download)
        .await
        .map_err(|_| Error::Timeout(limits.artifact_download_timeout))??;

    let digest = digest.finalize();
    let mut actual_sha256 = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual_sha256, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(Error::DownloadChecksumMismatch {
            url: display_url,
            expected: expected_sha256.to_string(),
            actual: actual_sha256,
        });
    }

    let extracted_destination = absolute_destination.clone();
    let staging_parent = destination_parent.to_path_buf();
    spawn_admitted(lease, move || {
        // Owned here so the archive is deleted, and its staging released,
        // only after extraction stops.
        let archive = archive;
        let staging = tempfile::Builder::new()
            .prefix(".once-download-")
            .tempdir_in(&staging_parent)
            .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: staging_parent.display().to_string(),
                source,
            })?;
        let staged_output = staging.path().join("output");
        extract_zip_archive(
            archive.file.path(),
            &staged_output,
            limits,
            expansion_bound(declared, limits),
        )?;
        remove_path_blocking(&extracted_destination).map_err(|source| Error::FileAction {
            action: "download_and_extract",
            path: extracted_destination.display().to_string(),
            source,
        })?;
        std::fs::rename(&staged_output, &extracted_destination).map_err(|source| {
            Error::FileAction {
                action: "download_and_extract",
                path: extracted_destination.display().to_string(),
                source,
            }
        })
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: destination.as_str().to_string(),
        source: std::io::Error::other(source.to_string()),
    })??;
    Ok(())
}

/// A downloaded archive and the staging reserved for it and its extracted
/// copy. Every blocking write and the extraction own a clone, so the file is
/// deleted and the reservation released only after the last task touching
/// it stops, extraction and publication included; on Windows an open handle
/// would otherwise keep the delete from succeeding.
struct DownloadedArchive {
    file: tempfile::NamedTempFile,
    _staging: Option<StagingPermit>,
}

/// The most bytes extraction may write for a download that declared
/// `declared` bytes on the wire: `max_archive_expansion_ratio` times that,
/// capped by the archive limit. A download that declares no length may
/// expand to the whole archive limit.
fn expansion_bound(declared: Option<u64>, limits: RemoteIoLimits) -> u64 {
    declared.map_or(limits.max_archive_bytes, |declared| {
        declared
            .saturating_mul(limits.max_archive_expansion_ratio)
            .min(limits.max_archive_bytes)
    })
}

/// Staging one download needs, reserved in a single all-or-nothing request
/// before anything is written, so the download never waits for a second
/// permit while it holds the first: the archive (its declared length, or the
/// archive limit when it declares none) plus its [`expansion_bound`].
/// Downloads that declare their length reserve in proportion to it and run
/// side by side. With the default limits a download that declares none
/// reserves the whole staging pool, so such downloads run one at a time. A
/// staging limit below this sum fails the download instead of starting it.
fn download_staging_bytes(declared: Option<u64>, limits: RemoteIoLimits) -> u64 {
    declared
        .unwrap_or(limits.max_archive_bytes)
        .saturating_add(expansion_bound(declared, limits))
}

/// Append `chunk` to the archive on the blocking pool. The write owns a
/// clone of the archive, its staging, and the lease, so cancelling the
/// caller releases none of them while the write is queued or running.
async fn write_archive_chunk(
    lease: Option<&ExecutionPermit>,
    archive: Arc<DownloadedArchive>,
    label: &str,
    chunk: impl AsRef<[u8]> + Send + 'static,
) -> Result<()> {
    file_step(lease, "download_and_extract", label, move || {
        archive.file.as_file().write_all(chunk.as_ref())
    })
    .await
}

fn check_download_size(size: u64, limits: RemoteIoLimits) -> Result<()> {
    if size > limits.max_archive_bytes {
        Err(Error::ExecutorResourceLimit {
            executor: Executor::Local.name().to_string(),
            resource: "download bytes",
            limit: limits.max_archive_bytes,
        })
    } else {
        Ok(())
    }
}

fn download_stalled(limits: RemoteIoLimits) -> Error {
    Error::ExecutorStalled {
        executor: Executor::Local.name().to_string(),
        phase: "download",
        timeout: limits.stall_timeout,
    }
}

/// `url` without user-info, query, or fragment, which can carry credentials
/// the caller owns and must not reach errors or logs.
fn redacted_url(url: &reqwest::Url) -> String {
    let mut redacted = url.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    if redacted.query().is_some() {
        redacted.set_query(Some("redacted"));
    }
    redacted.set_fragment(None);
    redacted.to_string()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Extract the archive at `archive_path` into `destination`, writing at
/// most `expansion` bytes of file contents.
fn extract_zip_archive(
    archive_path: &Path,
    destination: &Path,
    limits: RemoteIoLimits,
    expansion: u64,
) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: destination.display().to_string(),
        source,
    })?;
    let archive_file = std::fs::File::open(archive_path).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: archive_path.display().to_string(),
        source,
    })?;
    let mut archive = zip_preflight::open(archive_file, limits)?;
    if archive.len() > limits.max_archive_entries {
        return Err(Error::ExecutorResourceLimit {
            executor: Executor::Local.name().to_string(),
            resource: "archive entries",
            limit: u64::try_from(limits.max_archive_entries).unwrap_or(u64::MAX),
        });
    }
    let expanded_too_far = || Error::ExecutorResourceLimit {
        executor: Executor::Local.name().to_string(),
        resource: "extracted bytes",
        limit: expansion,
    };
    let mut remaining = expansion;
    for index in 0..archive.len() {
        let mut entry =
            archive
                .by_index(index)
                .map_err(|source| Error::InvalidDownloadAndExtract {
                    reason: format!("reading ZIP entry {index}: {source}"),
                })?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{}` escapes the destination", entry.name()),
            });
        };
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{}` escapes the destination", entry.name()),
            });
        }
        let output = destination.join(&relative);
        let unix_mode = entry.unix_mode().unwrap_or(0);
        let file_type = unix_mode & 0o170_000;
        if file_type != 0
            && file_type != 0o100_000
            && file_type != 0o040_000
            && file_type != 0o120_000
        {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{}` has an unsupported file type", entry.name()),
            });
        }
        if traverses_extracted_symlink(destination, &relative) {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!(
                    "ZIP entry `{}` is written through a symbolic link",
                    entry.name()
                ),
            });
        }
        if file_type == 0o120_000 {
            let mut target = String::new();
            let mut bounded = std::io::Read::take(&mut entry, limits.max_metadata_bytes);
            std::io::Read::read_to_string(&mut bounded, &mut target).map_err(|source| {
                Error::InvalidDownloadAndExtract {
                    reason: format!("reading ZIP symbolic link `{}`: {source}", entry.name()),
                }
            })?;
            extract_zip_symlink(&relative, Path::new(&target), &output).map_err(|reason| {
                Error::InvalidDownloadAndExtract {
                    reason: format!("ZIP entry `{}` {reason}", entry.name()),
                }
            })?;
            continue;
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
            continue;
        }
        // Refuse an entry that declares more than is left before creating
        // its file; `extract_bounded_file` never writes past the bound for
        // one that lies.
        if entry.size() > remaining {
            return Err(expanded_too_far());
        }
        let name = entry.name().to_string();
        let written = extract_bounded_file(&mut entry, &name, &output, unix_mode, remaining)?
            .ok_or_else(expanded_too_far)?;
        remaining -= written;
    }
    Ok(())
}

/// Extract one entry, writing at most `remaining` bytes. Returns `None` when
/// the entry holds more than that.
fn extract_bounded_file(
    entry: &mut impl std::io::Read,
    name: &str,
    output: &Path,
    unix_mode: u32,
    remaining: u64,
) -> Result<Option<u64>> {
    let written = extract_zip_file(
        &mut std::io::Read::take(&mut *entry, remaining),
        name,
        output,
        unix_mode,
    )?;
    let mut probe = [0_u8; 1];
    let more = entry.read(&mut probe).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: output.display().to_string(),
        source,
    })?;
    Ok((more == 0).then_some(written))
}

fn extract_zip_file(
    entry: &mut impl std::io::Read,
    name: &str,
    output: &Path,
    unix_mode: u32,
) -> Result<u64> {
    let parent = output
        .parent()
        .ok_or_else(|| Error::InvalidDownloadAndExtract {
            reason: format!("ZIP entry `{name}` has no parent directory"),
        })?;
    std::fs::create_dir_all(parent).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: parent.display().to_string(),
        source,
    })?;
    let mut output_file = std::fs::File::create(output).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: output.display().to_string(),
        source,
    })?;
    let written = std::io::copy(entry, &mut output_file).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: output.display().to_string(),
        source,
    })?;
    #[cfg(unix)]
    if unix_mode != 0 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(unix_mode & 0o777))
            .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
    }
    #[cfg(not(unix))]
    let _ = unix_mode;
    Ok(written)
}

/// Whether `relative`, or any directory above it, is a symbolic link that an
/// earlier entry of the same archive extracted under `destination`.
fn traverses_extracted_symlink(destination: &Path, relative: &Path) -> bool {
    let mut current = destination.to_path_buf();
    relative.components().any(|component| {
        current.push(component);
        std::fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink())
    })
}

/// Creates the symbolic link an archive stores at `relative`, such as the
/// `Versions/Current` links inside a macOS framework. The target must be
/// relative and stay inside the destination. `..` is accepted only before the
/// first named component, so it climbs real directories rather than out of a
/// directory reached through another link.
fn extract_zip_symlink(
    relative: &Path,
    target: &Path,
    output: &Path,
) -> std::result::Result<(), String> {
    let mut depth = relative.components().count().saturating_sub(1);
    let mut descended = false;
    for component in target.components() {
        match component {
            Component::Normal(_) => descended = true,
            Component::CurDir => {}
            Component::ParentDir if !descended && depth > 0 => depth -= 1,
            _ => {
                return Err(format!(
                    "links to `{}` outside the destination",
                    target.display()
                ))
            }
        }
    }
    let parent = output
        .parent()
        .ok_or_else(|| "has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|source| source.to_string())?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, output).map_err(|source| source.to_string())
    }
    #[cfg(not(unix))]
    {
        Err("is a symbolic link, which this platform cannot extract".to_string())
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_command(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
    observer: Option<&dyn ActionOutputObserver>,
    lease: Option<ExecutionPermit>,
    setup: remote::CommandFilesystemSetup<'_>,
) -> Result<(ActionResult, bool)> {
    let Action::RunCommand {
        argv,
        env,
        cwd,
        inputs,
        outputs,
        output_symlink_mode,
        resources,
        timeout_ms,
        success_exit_codes,
        remote,
        stdout_path,
        stderr_path,
        sandbox,
        network,
        ..
    } = action
    else {
        unreachable!("execute_command only accepts command actions")
    };

    let invocation = local::Invocation {
        argv,
        env,
        cwd: cwd.as_ref(),
        timeout_ms: *timeout_ms,
        redirect: local::Redirect {
            stdout: stdout_path.as_deref(),
            stderr: stderr_path.as_deref(),
        },
        network: *network,
    };

    let executor = Executor::resolve(action)?;
    if executor == Executor::Local {
        for (paths, mode) in [
            (setup.clean_paths, PreparePathMode::Remove),
            (setup.create_dirs, PreparePathMode::Directory),
        ] {
            for path in paths {
                execute_portable_action(
                    &Action::PreparePath {
                        path: path.clone(),
                        mode,
                        input_digest: None,
                    },
                    workspace_root,
                    cache,
                    lease.as_ref(),
                )
                .await?;
            }
        }
    }
    if executor != Executor::Local && validate_contract {
        return Err(executor.unsupported("filesystem contract auditing"));
    }
    if executor != Executor::Local && observer.is_some() {
        return Err(executor.unsupported("live output observers"));
    }
    let result = match (executor, stream_to_parent, sandbox) {
        (Executor::Microsandbox | Executor::E2b | Executor::Daytona | Executor::Docker, _, _) => {
            Box::pin(remote::execute_command(
                executor,
                remote.as_deref().expect("remote executor has a binding"),
                remote::PreparedCommand {
                    lease,
                    argv,
                    env,
                    cwd: cwd.as_ref(),
                    inputs,
                    outputs,
                    resources,
                    timeout_ms: *timeout_ms,
                    success_exit_codes,
                },
                workspace_root,
                cache,
                stream_to_parent,
                setup,
            ))
            .await
        }
        (Executor::Local, _, SandboxMode::Inputs | SandboxMode::CopiedInputs) => {
            return sandbox::execute_sandboxed_command(
                action,
                invocation,
                sandbox::SandboxPolicy {
                    inputs,
                    outputs,
                    output_symlink_mode: *output_symlink_mode,
                    copy_inputs: *sandbox == SandboxMode::CopiedInputs,
                    validate_contract,
                    lease: lease.as_ref(),
                },
                workspace_root,
                cache,
                stream_to_parent,
                observer,
            )
            .await
            .map(|result| (result, true));
        }
        (Executor::Local, _, SandboxMode::Off) => match observer {
            Some(observer) => {
                local::execute_command_observed(
                    invocation,
                    workspace_root,
                    cache,
                    stream_to_parent,
                    observer,
                )
                .await
            }
            None if stream_to_parent => {
                local::execute_command_streaming(invocation, workspace_root, cache).await
            }
            None => local::execute_command(invocation, workspace_root, cache).await,
        },
    }?;
    Ok((result, false))
}

fn copy_sandbox_output_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        copy_symlink_blocking(source, destination)
    } else if metadata.is_dir() {
        remove_path_blocking(destination)?;
        copy_directory_contents_blocking(source, destination)
    } else if metadata.is_file() {
        copy_file_blocking(source, destination)
    } else {
        Ok(())
    }
}

async fn capture_file_action_outputs(
    action_outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
    lease: Option<&ExecutionPermit>,
) -> Result<ActionResult> {
    let mut result = empty_file_action_result();
    result.outputs = outputs::capture_admitted(
        action_outputs,
        workspace_root,
        cache,
        OutputSymlinkMode::default(),
        lease,
    )
    .await?;
    Ok(result)
}

fn empty_file_action_result() -> ActionResult {
    ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::new(),
    }
}

/// Largest piece of a borrowed buffer copied into one blocking write, so a
/// write never clones the whole buffer.
const WRITE_CHUNK_BYTES: usize = 1 << 20;

async fn write_file(
    path: &WorkspacePath,
    bytes: &[u8],
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let absolute = path.resolve(workspace_root);
    if let Some(parent) = absolute.parent() {
        let parent = parent.to_path_buf();
        file_step(lease, "create_parent_dir", path.as_str(), move || {
            std::fs::create_dir_all(parent)
        })
        .await?;
    }
    let mut file = file_step(lease, "write_file", path.as_str(), move || {
        std::fs::File::create(absolute)
    })
    .await?;
    for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
        let chunk = chunk.to_vec();
        file = file_step(lease, "write_file", path.as_str(), move || {
            file.write_all(&chunk)?;
            Ok(file)
        })
        .await?;
    }
    Ok(())
}

async fn copy_file(
    source: &WorkspacePath,
    destination: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let absolute_source = source.resolve(workspace_root);
    let absolute_destination = destination.resolve(workspace_root);
    let destination_label = destination.as_str().to_string();
    spawn_admitted(lease, move || {
        let metadata = std::fs::metadata(&absolute_source)?;
        if let Some(parent) = absolute_destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        remove_path_blocking(&absolute_destination)?;
        if metadata.is_dir() {
            std::fs::create_dir_all(&absolute_destination)?;
            copy_directory_contents_blocking(&absolute_source, &absolute_destination)
        } else {
            std::fs::copy(&absolute_source, &absolute_destination).map(|_| ())
        }
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "copy_path",
        path: destination.as_str().to_string(),
        source: std::io::Error::other(source.to_string()),
    })?
    .map_err(|source| Error::FileAction {
        action: "copy_path",
        path: destination_label,
        source,
    })
}

async fn materialize_host_file(
    source: &Path,
    source_sha256: &str,
    destination: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    if !source.is_absolute() {
        return Err(Error::InvalidHostFile {
            reason: format!("source `{}` must be absolute", source.display()),
        });
    }
    let source = source.to_path_buf();
    let expected = source_sha256.to_string();
    let absolute = destination.resolve(workspace_root);
    let label = destination.as_str().to_string();
    let file_action = |action: &'static str, path: String| {
        move |source: std::io::Error| Error::FileAction {
            action,
            path,
            source,
        }
    };
    spawn_admitted(lease, move || {
        let bytes = std::fs::read(&source)
            .map_err(file_action("read_host_file", source.display().to_string()))?;
        let digest = sha2::Sha256::digest(&bytes);
        let mut actual = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut actual, "{byte:02x}").expect("writing to a String cannot fail");
        }
        if actual != expected {
            return Err(Error::HostFileDigestMismatch {
                path: source.display().to_string(),
                expected,
                actual,
            });
        }
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent)
                .map_err(file_action("create_parent_dir", label.clone()))?;
        }
        std::fs::write(&absolute, &bytes).map_err(file_action("write_file", label))
    })
    .await
    .map_err(|join| Error::FileAction {
        action: "materialize_host_file",
        path: destination.as_str().to_string(),
        source: std::io::Error::other(join.to_string()),
    })?
}

async fn materialize_host_tree(
    source: &Path,
    source_sha256: &str,
    destination: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    if !source.is_absolute() {
        return Err(Error::InvalidHostFile {
            reason: format!("source `{}` must be absolute", source.display()),
        });
    }
    if !source.is_dir() {
        return Err(Error::InvalidHostFile {
            reason: format!("source `{}` must be a directory", source.display()),
        });
    }
    let source = source.to_path_buf();
    let destination_path = destination.resolve(workspace_root);
    let destination_label = destination.as_str().to_string();
    let expected = source_sha256.to_string();
    spawn_admitted(lease, move || {
        // Copying a tree into itself (destination equal to, nested under, or an
        // ancestor of the source) would remove the verified source before
        // recreating it or recurse the fresh destination into itself, silently
        // destroying workspace data. Reject the overlap before touching disk.
        reject_overlapping_host_tree_paths(&source, &destination_path)?;
        let actual = once_host_tree::host_tree_sha256_hex(&source).map_err(|source_error| {
            Error::FileAction {
                action: "hash_host_tree",
                path: source.display().to_string(),
                source: source_error,
            }
        })?;
        if actual != expected {
            return Err(Error::HostFileDigestMismatch {
                path: source.display().to_string(),
                expected,
                actual,
            });
        }
        copy_sandbox_output_blocking(&source, &destination_path).map_err(|source_error| {
            Error::FileAction {
                action: "materialize_host_tree",
                path: destination_label,
                source: source_error,
            }
        })?;
        // The hash above and this copy are independent traversals of the
        // source. If the source changed in between, the destination we just
        // captured no longer matches `expected`; re-hash the copied snapshot
        // and reject it rather than caching bytes under a stale digest.
        let copied =
            once_host_tree::host_tree_sha256_hex(&destination_path).map_err(|source_error| {
                Error::FileAction {
                    action: "verify_host_tree",
                    path: destination_path.display().to_string(),
                    source: source_error,
                }
            })?;
        if copied != expected {
            return Err(Error::HostFileDigestMismatch {
                path: destination_path.display().to_string(),
                expected,
                actual: copied,
            });
        }
        Ok(())
    })
    .await
    .map_err(|source_error| Error::FileAction {
        action: "materialize_host_tree",
        path: destination.as_str().to_string(),
        source: std::io::Error::other(source_error.to_string()),
    })?
}

/// Reject a host-tree copy whose source and destination overlap: equal paths,
/// a destination nested under the source, or a source nested under the
/// destination. Symlinks in the existing prefixes are resolved so the check
/// cannot be bypassed by linking one side into the other.
fn reject_overlapping_host_tree_paths(source: &Path, destination: &Path) -> Result<()> {
    let source_c = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let dest_c = canonicalize_existing_prefix(destination);
    if dest_c == source_c || dest_c.starts_with(&source_c) || source_c.starts_with(&dest_c) {
        return Err(Error::InvalidHostFile {
            reason: format!(
                "materialize_host_tree destination `{}` overlaps source `{}`",
                destination.display(),
                source.display()
            ),
        });
    }
    Ok(())
}

/// Canonicalize the longest existing prefix of `path` (resolving any symlinks
/// there) and re-append the not-yet-created remainder lexically, so a
/// destination that does not exist on disk can still be compared safely.
pub(crate) fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut resolved = canonical;
            for part in suffix.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                suffix.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

async fn copy_tree(
    sources: &[WorkspacePath],
    destination: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let source_paths = sources
        .iter()
        .map(|source| source.resolve(workspace_root))
        .collect::<Vec<_>>();
    let destination_path = destination.resolve(workspace_root);
    let destination_label = destination.as_str().to_string();
    spawn_admitted(lease, move || {
        if destination_path.exists() {
            remove_path_blocking(&destination_path).map_err(|source| Error::FileAction {
                action: "remove_path",
                path: destination_label.clone(),
                source,
            })?;
        }
        std::fs::create_dir_all(&destination_path).map_err(|source| Error::FileAction {
            action: "ensure_dir",
            path: destination_label.clone(),
            source,
        })?;
        for source_path in source_paths {
            copy_tree_contents_blocking(&source_path, &destination_path).map_err(|source| {
                Error::FileAction {
                    action: "copy_tree",
                    path: destination_label.clone(),
                    source,
                }
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "copy_tree",
        path: destination.as_str().to_string(),
        source: std::io::Error::other(source.to_string()),
    })?
}

async fn remove_path(
    path: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let absolute = path.resolve(workspace_root);
    file_step(lease, "remove_path", path.as_str(), move || {
        let metadata = match std::fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(source),
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            std::fs::remove_dir_all(&absolute)
        } else {
            std::fs::remove_file(&absolute)
        }
    })
    .await
}

async fn ensure_dir(
    path: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let absolute = path.resolve(workspace_root);
    file_step(lease, "ensure_dir", path.as_str(), move || {
        std::fs::create_dir_all(absolute)
    })
    .await
}

async fn link_path(
    source: &WorkspacePath,
    destination: &WorkspacePath,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    if Path::new(source.as_str()).starts_with(Path::new(destination.as_str())) {
        return Err(Error::InvalidLinkPath {
            reason: format!(
                "destination `{destination}` must not equal or contain source `{source}`"
            ),
        });
    }
    let relative_target = relative_workspace_link_target(source, destination);
    let source = source.resolve(workspace_root);
    let destination = destination.resolve(workspace_root);
    let source_for_link = source.clone();
    let destination_for_link = destination.clone();
    spawn_admitted(lease, move || {
        std::fs::symlink_metadata(&source_for_link)?;
        if let Some(parent) = destination_for_link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        remove_path_blocking(&destination_for_link)?;
        create_symlink_blocking(&relative_target, &destination_for_link, &source_for_link)
    })
    .await
    .map_err(|source_error| Error::FileAction {
        action: "link_path",
        path: destination.display().to_string(),
        source: std::io::Error::other(source_error.to_string()),
    })?
    .map_err(|source_error| Error::FileAction {
        action: "link_path",
        path: destination.display().to_string(),
        source: source_error,
    })
}

fn relative_workspace_link_target(source: &WorkspacePath, destination: &WorkspacePath) -> PathBuf {
    let source_parts = source
        .as_str()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let destination_parent = Path::new(destination.as_str())
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let destination_parts = destination_parent
        .to_string_lossy()
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let common = source_parts
        .iter()
        .zip(&destination_parts)
        .take_while(|(source, destination)| source == destination)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..destination_parts.len() {
        relative.push("..");
    }
    for part in &source_parts[common..] {
        relative.push(part);
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    relative
}

async fn write_tree_digest(
    root: &WorkspacePath,
    output: &WorkspacePath,
    include_suffixes: &[String],
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let root_path = root.resolve(workspace_root);
    let output_path = output.resolve(workspace_root);
    let output_label = output.as_str().to_string();
    let suffixes = include_suffixes.to_vec();
    spawn_admitted(lease, move || {
        let file_action = |action: &'static str| {
            let path = output_label.clone();
            move |source: std::io::Error| Error::FileAction {
                action,
                path,
                source,
            }
        };
        let bytes =
            tree_digest_bytes(&root_path, &suffixes).map_err(file_action("write_tree_digest"))?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(file_action("create_parent_dir"))?;
        }
        std::fs::write(&output_path, bytes).map_err(file_action("write_tree_digest"))
    })
    .await
    .map_err(|join| Error::FileAction {
        action: "write_tree_digest",
        path: output.as_str().to_string(),
        source: std::io::Error::other(join.to_string()),
    })?
}

fn remove_path_blocking(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(source),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        remove_tree_blocking(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Remove a directory tree, first restoring owner access to directories whose
/// mode forbids deleting their entries. Sandboxes mirror source permissions,
/// so a read-only source directory must not make its copy undeletable.
fn remove_tree_blocking(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(source) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            make_tree_removable(path)?;
            std::fs::remove_dir_all(path)
        }
        other => other,
    }
}

fn make_tree_removable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o700);
            std::fs::set_permissions(path, permissions)?;
            for entry in std::fs::read_dir(path)? {
                make_tree_removable(&entry?.path())?;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn copy_tree_contents_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let file_name = source.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source `{}` has no file name", source.display()),
            )
        })?;
        copy_symlink_blocking(source, &destination.join(file_name))?;
        return Ok(());
    }
    if metadata.is_file() {
        let file_name = source.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source `{}` has no file name", source.display()),
            )
        })?;
        copy_file_blocking(source, &destination.join(file_name))?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    copy_directory_contents_blocking(source, destination)
}

fn copy_directory_contents_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    let mut children = std::fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let child_path = child.path();
        let child_destination = destination.join(child.file_name());
        let metadata = std::fs::symlink_metadata(&child_path)?;
        if metadata.file_type().is_symlink() {
            copy_symlink_blocking(&child_path, &child_destination)?;
        } else if metadata.is_dir() {
            copy_directory_contents_blocking(&child_path, &child_destination)?;
        } else if metadata.is_file() {
            copy_file_blocking(&child_path, &child_destination)?;
        }
    }
    std::fs::set_permissions(
        destination,
        std::fs::symlink_metadata(source)?.permissions(),
    )?;
    Ok(())
}

fn copy_symlink_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(source)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_blocking(destination)?;
    create_symlink_blocking(&target, destination, source)
}

#[cfg(unix)]
fn create_symlink_blocking(
    target: &Path,
    destination: &Path,
    _source: &Path,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn create_symlink_blocking(
    target: &Path,
    destination: &Path,
    source: &Path,
) -> std::io::Result<()> {
    let target_is_dir = std::fs::metadata(source)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    if target_is_dir {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

fn copy_file_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_blocking(destination)?;
    std::fs::copy(source, destination)?;
    // The platform copy does not always carry the setuid, setgid, and sticky
    // bits, so set the source's permissions explicitly.
    std::fs::set_permissions(destination, std::fs::metadata(source)?.permissions())?;
    Ok(())
}

fn tree_digest_bytes(root: &Path, include_suffixes: &[String]) -> std::io::Result<Vec<u8>> {
    let metadata = match std::fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(source),
    };
    if !metadata.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect_tree_files(root, root, include_suffixes, &mut files)?;
    files.sort();
    let mut out = Vec::new();
    for relative in files {
        let absolute = root.join(&relative);
        let digest = file_sha256_hex(&absolute)?;
        out.extend_from_slice(digest.as_bytes());
        out.extend_from_slice(b"  ");
        out.extend_from_slice(relative.to_string_lossy().replace('\\', "/").as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

fn collect_tree_files(
    root: &Path,
    dir: &Path,
    include_suffixes: &[String],
    files: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    let mut children = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_dir() {
            collect_tree_files(root, &path, include_suffixes, files)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        if include_suffixes.is_empty()
            || include_suffixes
                .iter()
                .any(|suffix| relative_str.ends_with(suffix))
        {
            files.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        copy_tree_contents_blocking, extract_zip_archive, file_sha256_hex, hex_lower,
        tree_digest_bytes,
    };

    enum ZipFixtureEntry<'a> {
        File(&'a str, &'a str),
        Symlink(&'a str, &'a str),
    }

    fn write_zip(path: &std::path::Path, entries: &[ZipFixtureEntry<'_>]) {
        let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for entry in entries {
            match entry {
                ZipFixtureEntry::File(name, contents) => {
                    archive.start_file(*name, options).unwrap();
                    std::io::Write::write_all(&mut archive, contents.as_bytes()).unwrap();
                }
                ZipFixtureEntry::Symlink(name, target) => {
                    archive.add_symlink(*name, *target, options).unwrap();
                }
            }
        }
        archive.finish().unwrap();
    }

    fn extract_fixture(entries: &[ZipFixtureEntry<'_>]) -> (tempfile::TempDir, crate::Result<()>) {
        extract_fixture_within(entries, crate::RemoteIoLimits::default())
    }

    fn extract_fixture_within(
        entries: &[ZipFixtureEntry<'_>],
        limits: crate::RemoteIoLimits,
    ) -> (tempfile::TempDir, crate::Result<()>) {
        let tmp = tempfile::TempDir::new().unwrap();
        let archive = tmp.path().join("fixture.zip");
        write_zip(&archive, entries);
        let result = extract_zip_archive(
            &archive,
            &tmp.path().join("output"),
            limits,
            limits.max_archive_bytes,
        );
        (tmp, result)
    }

    #[test]
    fn extraction_stops_at_the_extracted_byte_limit() {
        use ZipFixtureEntry::File;
        let limits = crate::RemoteIoLimits {
            max_archive_bytes: 8,
            ..crate::RemoteIoLimits::default()
        };
        let (_tmp, within) =
            extract_fixture_within(&[File("a", "1234"), File("b", "5678")], limits);
        within.unwrap();
        let (_tmp, over) = extract_fixture_within(&[File("a", "1234"), File("b", "56789")], limits);
        assert!(matches!(
            over,
            Err(crate::Error::ExecutorResourceLimit {
                resource: "extracted bytes",
                ..
            })
        ));
    }

    #[test]
    fn extraction_refuses_archives_with_too_many_entries() {
        use ZipFixtureEntry::File;
        let limits = crate::RemoteIoLimits {
            max_archive_entries: 1,
            ..crate::RemoteIoLimits::default()
        };
        let (_tmp, result) = extract_fixture_within(&[File("a", "1"), File("b", "2")], limits);
        assert!(matches!(
            result,
            Err(crate::Error::ExecutorResourceLimit {
                resource: "archive entries",
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn extract_zip_archive_restores_framework_symlinks() {
        use ZipFixtureEntry::{File, Symlink};
        let (tmp, result) = extract_fixture(&[
            File("Vendor.framework/Versions/A/Vendor", "binary"),
            File("Vendor.framework/Versions/A/Resources/Info.plist", "plist"),
            Symlink("Vendor.framework/Versions/Current", "A"),
            Symlink("Vendor.framework/Vendor", "Versions/Current/Vendor"),
            Symlink("Vendor.framework/Resources", "Versions/Current/Resources"),
            Symlink("Vendor.framework/Sibling", "../Other"),
        ]);
        result.unwrap();
        let framework = tmp.path().join("output/Vendor.framework");
        assert_eq!(
            std::fs::read_link(framework.join("Resources")).unwrap(),
            std::path::Path::new("Versions/Current/Resources")
        );
        assert_eq!(
            std::fs::read_to_string(framework.join("Resources/Info.plist")).unwrap(),
            "plist"
        );
        assert_eq!(
            std::fs::read_to_string(framework.join("Vendor")).unwrap(),
            "binary"
        );
    }

    #[test]
    fn extract_zip_archive_rejects_symlinks_that_leave_the_destination() {
        use ZipFixtureEntry::{File, Symlink};
        for entries in [
            vec![Symlink("Vendor/escape", "/etc")],
            vec![Symlink("Vendor/escape", "../../outside")],
            vec![Symlink("escape", "..")],
            vec![Symlink("root", "."), Symlink("Vendor/escape", "../root/..")],
            vec![Symlink("root", "."), Symlink("root/escape", "..")],
            vec![Symlink("root", "."), File("root/file", "contents")],
        ] {
            let (tmp, result) = extract_fixture(&entries);
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("outside the destination")
                    || error.contains("through a symbolic link"),
                "{error}"
            );
            assert!(!tmp.path().join("outside").exists());
        }
    }

    fn tree_digest_string(root: &std::path::Path, suffixes: &[String]) -> String {
        String::from_utf8(tree_digest_bytes(root, suffixes).unwrap()).unwrap()
    }

    #[test]
    fn hex_lower_pads_each_byte_to_two_digits() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
        assert_eq!(hex_lower(&[]), "");
    }

    #[test]
    fn file_sha256_hex_matches_known_vector() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("hello.txt");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(
            file_sha256_hex(&path).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn tree_digest_is_empty_for_missing_or_non_directory_roots() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(tree_digest_bytes(&tmp.path().join("absent"), &[])
            .unwrap()
            .is_empty());
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(tree_digest_bytes(&file, &[]).unwrap().is_empty());
    }

    #[test]
    fn tree_digest_lists_files_sorted_and_relative() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join("sub").join("c.txt"), b"c").unwrap();

        let rendered = tree_digest_string(tmp.path(), &[]);
        let paths: Vec<&str> = rendered
            .lines()
            .map(|line| line.rsplit("  ").next().unwrap())
            .collect();
        assert_eq!(paths, ["a.txt", "b.txt", "sub/c.txt"]);
    }

    #[test]
    fn tree_digest_honors_suffix_filter() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("keep.txt"), b"1").unwrap();
        std::fs::write(tmp.path().join("skip.rs"), b"2").unwrap();

        let rendered = tree_digest_string(tmp.path(), &[".txt".to_string()]);
        assert!(rendered.contains("keep.txt"));
        assert!(!rendered.contains("skip.rs"));
    }

    #[test]
    fn tree_digest_is_independent_of_creation_order() {
        let first = tempfile::TempDir::new().unwrap();
        std::fs::write(first.path().join("a"), b"one").unwrap();
        std::fs::write(first.path().join("b"), b"two").unwrap();

        let second = tempfile::TempDir::new().unwrap();
        std::fs::write(second.path().join("b"), b"two").unwrap();
        std::fs::write(second.path().join("a"), b"one").unwrap();

        assert_eq!(
            tree_digest_bytes(first.path(), &[]).unwrap(),
            tree_digest_bytes(second.path(), &[]).unwrap()
        );
    }

    #[test]
    fn copy_tree_contents_preserves_nested_files() {
        let source = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(source.path().join("nested")).unwrap();
        std::fs::write(source.path().join("top.txt"), b"top").unwrap();
        std::fs::write(source.path().join("nested").join("deep.txt"), b"deep").unwrap();

        let dest = tempfile::TempDir::new().unwrap();
        copy_tree_contents_blocking(source.path(), dest.path()).unwrap();

        assert_eq!(std::fs::read(dest.path().join("top.txt")).unwrap(), b"top");
        assert_eq!(
            std::fs::read(dest.path().join("nested").join("deep.txt")).unwrap(),
            b"deep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_contents_preserves_the_host_tree_digest() {
        use std::os::unix::fs::PermissionsExt;

        let source = tempfile::TempDir::new().unwrap();
        let nested = source.path().join("Framework.framework");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o775)).unwrap();
        let executable = nested.join("Framework");
        std::fs::write(&executable, b"binary").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let destination = tempfile::TempDir::new().unwrap();
        copy_tree_contents_blocking(source.path(), destination.path()).unwrap();

        assert_eq!(
            once_host_tree::host_tree_sha256_hex(source.path()).unwrap(),
            once_host_tree::host_tree_sha256_hex(destination.path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_contents_preserves_symlinks() {
        let source = tempfile::TempDir::new().unwrap();
        std::fs::write(source.path().join("target.txt"), b"data").unwrap();
        std::os::unix::fs::symlink("target.txt", source.path().join("link.txt")).unwrap();

        let dest = tempfile::TempDir::new().unwrap();
        copy_tree_contents_blocking(source.path(), dest.path()).unwrap();

        let copied = dest.path().join("link.txt");
        assert!(std::fs::symlink_metadata(&copied)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(&copied).unwrap(),
            std::path::Path::new("target.txt")
        );
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::redacted_url;

    #[test]
    fn download_urls_drop_credentials_before_reaching_errors() {
        let url = reqwest::Url::parse(
            "https://user:secret@example.com/archive.zip?X-Amz-Signature=abc&token=t#frag",
        )
        .unwrap();
        let redacted = redacted_url(&url);
        assert_eq!(redacted, "https://example.com/archive.zip?redacted");
        for secret in ["user", "secret", "abc", "token", "frag"] {
            assert!(!redacted.contains(secret), "{redacted} leaks {secret}");
        }
    }

    #[test]
    fn download_urls_without_credentials_are_unchanged() {
        let url = reqwest::Url::parse("https://example.com/a/b.zip").unwrap();
        assert_eq!(redacted_url(&url), "https://example.com/a/b.zip");
    }
}
