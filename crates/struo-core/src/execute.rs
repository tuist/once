use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::Digest as ShaDigest;
use std::io::Read as _;
use std::io::Write as _;
use struo_cas::{ActionResult, CacheProvider};

mod download;
mod fs;
mod sandbox;

#[cfg(all(test, unix))]
mod admission_tests;
#[cfg(test)]
mod tests;

use download::download_and_extract;
use fs::{
    copy_directory_contents_blocking, copy_file_blocking, copy_symlink_blocking,
    copy_tree_contents_blocking, create_symlink_blocking, remove_path_blocking, tree_digest_bytes,
};

use crate::stream::ActionOutputObserver;
use crate::{
    archive, local, outputs, remote, Action, CopyPathMode, Error, ExecutionPermit, Executor,
    OutputSymlinkMode, PreparePathMode, Result, SandboxMode, WorkspacePath,
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
                CopyPathMode::Files { strip_prefix } => {
                    let destinations =
                        crate::action::copy_files_destinations(sources, destination, strip_prefix)
                            .map_err(|reason| Error::InvalidCopyPath { reason })?;
                    for (source, target) in sources.iter().zip(&destinations) {
                        copy_file(source, target, workspace_root, lease).await?;
                    }
                    return capture_file_action_outputs(
                        &destinations,
                        workspace_root,
                        cache,
                        lease,
                    )
                    .await;
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
            source_mode,
            destination,
            ..
        } => {
            materialize_host_file(
                source,
                source_sha256,
                *source_mode,
                destination,
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
                prepare_path(path, mode, workspace_root, lease.as_ref()).await?;
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
        (
            Executor::Microsandbox
            | Executor::E2b
            | Executor::Daytona
            | Executor::Docker
            | Executor::Tuist,
            _,
            _,
        ) => {
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
                    redirect: invocation.redirect,
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
    Ok((result, executor == Executor::Tuist))
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

pub(crate) async fn prepare_path(
    path: &WorkspacePath,
    mode: PreparePathMode,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    match mode {
        PreparePathMode::Remove => remove_path(path, workspace_root, lease).await,
        PreparePathMode::Directory => ensure_dir(path, workspace_root, lease).await,
    }
}

async fn write_file(
    path: &WorkspacePath,
    bytes: &bytes::Bytes,
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
    let bytes = bytes.clone();
    file_step(lease, "write_file", path.as_str(), move || {
        std::fs::write(absolute, bytes)
    })
    .await
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
    source_mode: Option<u32>,
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
        let mut input = std::fs::File::open(&source)
            .map_err(file_action("read_host_file", source.display().to_string()))?;
        let metadata = input
            .metadata()
            .map_err(file_action("stat_host_file", source.display().to_string()))?;
        if source_mode.is_some_and(|mode| struo_host_tree::host_file_mode(&metadata) != mode) {
            return Err(Error::InvalidHostFile {
                reason: format!(
                    "source `{}` permissions changed after analysis",
                    source.display()
                ),
            });
        }
        let parent = absolute.parent().expect("workspace file has a parent");
        std::fs::create_dir_all(parent).map_err(file_action("create_parent_dir", label.clone()))?;
        let mut output = tempfile::NamedTempFile::new_in(parent)
            .map_err(file_action("create_host_file_snapshot", label.clone()))?;
        let mut hasher = sha2::Sha256::new();
        let mut buffer = [0; 16 * 1024];
        loop {
            let count = input
                .read(&mut buffer)
                .map_err(file_action("read_host_file", source.display().to_string()))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(file_action("write_host_file_snapshot", label.clone()))?;
        }
        let digest = hasher.finalize();
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
        if source_mode.is_none() {
            // Legacy actions retain ordinary file-creation permissions.
            let mut snapshot = output
                .reopen()
                .map_err(file_action("read_host_file_snapshot", label.clone()))?;
            let mut destination = std::fs::File::create(&absolute)
                .map_err(file_action("write_file", label.clone()))?;
            return std::io::copy(&mut snapshot, &mut destination)
                .map(|_| ())
                .map_err(file_action("write_file", label));
        }
        output
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(file_action("set_host_file_permissions", label.clone()))?;
        output
            .persist(&absolute)
            .map(|_| ())
            .map_err(|error| file_action("write_file", label)(error.error))
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
        let actual = struo_host_tree::host_tree_sha256_hex(&source).map_err(|source_error| {
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
        copy_sandbox_output_blocking(&source, &destination_path)
            .and_then(|()| {
                struo_host_tree::copy_escaping_symlink_targets(&source, &destination_path)
            })
            .map_err(|source_error| Error::FileAction {
                action: "materialize_host_tree",
                path: destination_label,
                source: source_error,
            })?;
        // The hash above and this copy are independent traversals of the
        // source. If the source changed in between, the destination we just
        // captured no longer matches `expected`; re-hash the copied snapshot
        // and reject it rather than caching bytes under a stale digest.
        let copied =
            struo_host_tree::host_tree_sha256_hex(&destination_path).map_err(|source_error| {
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
