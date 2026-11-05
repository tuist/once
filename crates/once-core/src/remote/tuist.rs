//! Execution through a Tuist project's Remote Execution API service.
//!
//! Inputs become a content-addressed tree in the project's CAS; only blobs the
//! CAS is missing are uploaded. Outputs are read back from that CAS by digest
//! and installed through the same validated staging as other adapters.
//!
//! The service may run an action more than once, so only duplicate-safe
//! actions should be placed here.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use once_cas::{
    reapi, reapi_protobuf, ActionResult, CacheProvider, ReapiMessage as _, TuistExecution,
    DEFAULT_EXECUTION_TIMEOUT,
};

use super::output_install::OutputStaging;
use super::path::Directory;
use super::{CommandFilesystemSetup, PreparedCommand};
use crate::{
    resolve_execution_argv, resolve_execution_env, Error, Executor, RemoteExecution,
    RemoteIoLimits, Result, WorkspacePath,
};

mod tree;

pub(super) const PROVIDER: &str = "tuist";
const PARALLEL_TRANSFERS: usize = 8;
/// Tuist workers refuse actions that declare a longer timeout.
const MAX_TIMEOUT: Duration = Duration::from_mins(10);
/// Largest output tree a Tuist worker returns.
const MAX_OUTPUT_TREE_BYTES: u64 = 4 * 1024 * 1024;
/// Search path for commands whose environment declares none. Workers resolve
/// a bare program name only through the action's own `PATH`.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub(super) async fn execute_command(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace: &Path,
    cache: &CacheProvider,
    parent: bool,
    setup: CommandFilesystemSetup<'_>,
) -> Result<ActionResult> {
    let execution = connect(remote, cache).await?;
    let limits = command
        .lease
        .as_ref()
        .map_or_else(RemoteIoLimits::default, crate::ExecutionPermit::io_limits);
    let cwd = command.cwd.map_or("", WorkspacePath::as_str);
    let (action_digest, input_tree, timeout) =
        prepare(&execution, remote, &command, setup, workspace, limits).await?;
    let lease = command.lease.as_ref();
    let capture = lease.map(crate::ExecutionPermit::capture_budget);
    if let Some(capture) = &capture {
        capture.reserve_upfront().await?;
    }
    let slot = execution.admit().await;
    upload_missing(&execution, &input_tree, workspace, lease).await?;
    tracing::debug!(executor = PROVIDER, action_digest = %action_digest.hash, "executing remote action");

    let executed = match execution.execute(&action_digest, timeout).await {
        Err(error) if error.is_not_found() => {
            tracing::debug!(
                executor = PROVIDER,
                "inputs left the execution CAS before the action ran; uploading them again"
            );
            upload_missing(&execution, &input_tree, workspace, lease).await?;
            execution.execute(&action_digest, timeout).await?
        }
        executed => executed?,
    };
    // Downloading results does not occupy the account's execution capacity.
    drop(slot);
    let result = executed.result;
    let stdout = mirror_log(
        &execution,
        cache,
        result.stdout_digest.as_ref(),
        &result.stdout_raw,
        limits,
    )
    .await?;
    let stderr = mirror_log(
        &execution,
        cache,
        result.stderr_digest.as_ref(),
        &result.stderr_raw,
        limits,
    )
    .await?;
    if parent || executed.timed_out {
        for (digest, stream) in [(&stdout, Stream::Stdout), (&stderr, Stream::Stderr)] {
            if let Some(digest) = digest {
                forward(cache, workspace, digest, stream).await?;
            }
        }
    }
    if executed.timed_out {
        return Err(Error::Timeout(timeout.unwrap_or(DEFAULT_EXECUTION_TIMEOUT)));
    }
    if command.accepts_exit_code(result.exit_code) && !command.outputs.is_empty() {
        install_outputs(&execution, cache, workspace, &command, cwd, &result, limits).await?;
    }
    tracing::debug!(
        executor = PROVIDER,
        exit_code = result.exit_code,
        "remote action completed"
    );
    Ok(ActionResult {
        exit_code: result.exit_code,
        stdout,
        stderr,
        outputs: std::collections::BTreeMap::new(),
    })
}

async fn connect(remote: &RemoteExecution, cache: &CacheProvider) -> Result<TuistExecution> {
    let config_error = |message: String| Error::RemoteProviderConfig {
        provider: remote.provider.clone(),
        message,
    };
    let CacheProvider::Tuist(tuist) = cache else {
        return Err(config_error(
            "Tuist remote execution reads inputs from the project's Tuist cache; set infrastructure.cache to the same Tuist infrastructure".into(),
        ));
    };
    for (field, expected, actual) in [
        ("account", &remote.account, &tuist.config().account),
        ("project", &remote.project, &tuist.config().project),
    ] {
        if let Some(expected) = expected {
            if actual.as_ref() != Some(expected) {
                return Err(config_error(format!(
                    "execution targets {field} `{expected}` but the cache uses {field} `{}`; both must name the same Tuist {field}",
                    actual.as_deref().unwrap_or("none")
                )));
            }
        }
    }
    let execution = tuist.execution().await?;
    let image = remote.environment.as_deref().unwrap_or_default();
    if image != execution.image() {
        return Err(config_error(format!(
            "execution_image `{image}` does not match the image `{}` the project's execution service runs; set execution_image = \"{}\"",
            execution.image(),
            execution.image()
        )));
    }
    Ok(execution)
}

/// Encode the action and its input tree; returns the action digest, every
/// blob the action references, and its timeout.
async fn prepare(
    execution: &TuistExecution,
    remote: &RemoteExecution,
    command: &PreparedCommand<'_>,
    setup: CommandFilesystemSetup<'_>,
    workspace: &Path,
    limits: RemoteIoLimits,
) -> Result<(reapi::Digest, tree::InputTree, Option<Duration>)> {
    let image = remote.environment.as_deref().unwrap_or_default();
    let cwd = command.cwd.map_or("", WorkspacePath::as_str);
    let root = Path::new(execution.execution_root());
    let mut output_paths = command
        .outputs
        .iter()
        .map(|output| relative_to(cwd, output.as_str()))
        .collect::<Result<Vec<_>>>()?;
    output_paths.sort();
    output_paths.dedup();
    let arguments = resolve_execution_argv(command.argv, root);
    if arguments.is_empty() {
        return Err(Error::EmptyArgv);
    }
    let mut environment = resolve_execution_env(command.env, root);
    environment
        .entry("PATH".into())
        .or_insert_with(|| DEFAULT_PATH.into());
    let remote_command = reapi::Command {
        arguments,
        environment_variables: environment
            .into_iter()
            .map(|(name, value)| reapi::command::EnvironmentVariable { name, value })
            .collect(),
        output_paths,
        working_directory: cwd.to_string(),
        ..Default::default()
    };

    let mut directories = setup
        .create_dirs
        .iter()
        .map(|path| path.as_str().to_string())
        .collect::<Vec<_>>();
    directories.extend(command.cwd.map(|cwd| cwd.as_str().to_string()));
    let (inputs, clean_paths, restored) = (
        command.inputs.to_vec(),
        setup.clean_paths.to_vec(),
        setup.post_setup_inputs.to_vec(),
    );
    let tree_workspace = workspace.to_path_buf();
    let mut input_tree = tokio::task::spawn_blocking(move || {
        tree::build(
            &tree_workspace,
            &tree::Layout {
                inputs: &inputs,
                clean_paths: &clean_paths,
                restored_inputs: &restored,
                directories: directories.iter().map(String::as_str).collect(),
            },
            limits,
        )
    })
    .await
    .map_err(|source| tree::api_error(format!("input tree task failed: {source}")))??;

    let timeout = command.timeout_ms.map(Duration::from_millis);
    if timeout.is_some_and(|timeout| timeout > MAX_TIMEOUT) {
        return Err(Executor::Tuist.unsupported("action timeouts longer than ten minutes"));
    }
    let command_bytes = remote_command.encode_to_vec();
    let command_digest = tree::digest_bytes(&command_bytes);
    let action = reapi::Action {
        command_digest: Some(command_digest.clone()),
        input_root_digest: Some(input_tree.root.clone()),
        timeout: timeout.map(|timeout| reapi_protobuf::Duration {
            seconds: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
            nanos: i32::try_from(timeout.subsec_nanos()).unwrap_or_default(),
        }),
        platform: Some(reapi::Platform {
            properties: [
                ("Arch", "amd64"),
                ("OSFamily", "Linux"),
                ("tuist-image", image),
            ]
            .into_iter()
            .map(|(name, value)| reapi::platform::Property {
                name: name.into(),
                value: value.into(),
            })
            .collect(),
        }),
        ..Default::default()
    };
    let action_bytes = action.encode_to_vec();
    let action_digest = tree::digest_bytes(&action_bytes);
    for (digest, bytes) in [
        (command_digest, command_bytes),
        (action_digest.clone(), action_bytes),
    ] {
        input_tree
            .blobs
            .insert(tree::key(&digest), (digest, tree::Blob::Bytes(bytes)));
    }
    Ok((action_digest, input_tree, timeout))
}

/// REAPI output paths are relative to the working directory, and Tuist
/// workers refuse paths that leave it.
fn relative_to(cwd: &str, output: &str) -> Result<String> {
    if cwd.is_empty() {
        return Ok(output.to_string());
    }
    output
        .strip_prefix(cwd)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            Executor::Tuist.unsupported("declared outputs outside the command working directory")
        })
}

async fn upload_missing(
    execution: &TuistExecution,
    tree: &tree::InputTree,
    workspace: &Path,
    lease: Option<&crate::ExecutionPermit>,
) -> Result<()> {
    let digests = tree
        .blobs
        .values()
        .map(|(digest, _)| digest.clone())
        .collect();
    let missing = execution.find_missing(digests).await?;
    let pending = missing
        .iter()
        .filter_map(|digest| tree.blobs.get(&tree::key(digest)))
        // Directory and command blobs are small; files are only paths.
        .map(|(digest, blob)| (digest.clone(), blob.clone()))
        .collect::<Vec<_>>();
    tracing::debug!(
        executor = PROVIDER,
        missing = pending.len(),
        "uploading missing remote inputs"
    );
    futures::stream::iter(pending)
        .map(|(digest, blob)| {
            async move {
                match blob {
                    tree::Blob::File(relative) => {
                        let size = u64::try_from(digest.size_bytes).unwrap_or(u64::MAX);
                        let _phase = super::phase_permit(lease, size).await?;
                        let copy = verified_copy(workspace, relative, &digest).await?;
                        Ok(execution.upload_file(&digest, copy.path()).await?)
                    }
                    tree::Blob::Bytes(bytes) => Ok(execution.upload_blob(&digest, &bytes).await?),
                }
            }
            .boxed()
        })
        .buffer_unordered(PARALLEL_TRANSFERS)
        .try_collect::<()>()
        .await
}

/// Copy an input through a handle that never follows links, and refuse it if
/// its bytes no longer match the digest the input tree was built from.
async fn verified_copy(
    workspace: &Path,
    relative: PathBuf,
    digest: &reapi::Digest,
) -> Result<tempfile::NamedTempFile> {
    let workspace = workspace.to_path_buf();
    let expected = digest.clone();
    tokio::task::spawn_blocking(move || {
        let io = |action, source| file_error(action, &relative, source);
        let mut source = Directory::open(&workspace)
            .and_then(|directory| directory.open_file(&relative))
            .map_err(|source| io("open_remote_input", source))?;
        let scratch = workspace.join(".once/tmp");
        std::fs::create_dir_all(&scratch).map_err(|source| io("stage_remote_input", source))?;
        let mut copy = tempfile::NamedTempFile::new_in(scratch)
            .map_err(|source| io("stage_remote_input", source))?;
        let mut tee = Tee(&mut source, copy.as_file_mut());
        let actual =
            tree::hash_reader(&mut tee).map_err(|source| io("stage_remote_input", source))?;
        if actual != expected {
            return Err(tree::api_error(format!(
                "input `{}` changed while it was uploaded; build again once it is stable",
                relative.display()
            )));
        }
        Ok(copy)
    })
    .await
    .map_err(|source| tree::api_error(format!("input staging task failed: {source}")))?
}

/// Reads from the first, writing everything read into the second.
struct Tee<'a, R, W>(&'a mut R, &'a mut W);

impl<R: std::io::Read, W: std::io::Write> std::io::Read for Tee<'_, R, W> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.0.read(buffer)?;
        self.1.write_all(&buffer[..read])?;
        Ok(read)
    }
}

async fn mirror_log(
    execution: &TuistExecution,
    cache: &CacheProvider,
    digest: Option<&reapi::Digest>,
    raw: &[u8],
    limits: RemoteIoLimits,
) -> Result<Option<once_cas::Digest>> {
    let size = digest.map_or(raw.len() as u64, |digest| {
        u64::try_from(digest.size_bytes).unwrap_or(u64::MAX)
    });
    if size > limits.max_capture_bytes {
        return Err(Error::ExecutorResourceLimit {
            executor: PROVIDER.into(),
            resource: "captured output",
            limit: limits.max_capture_bytes,
        });
    }
    if let Some(digest) = digest {
        return Ok(Some(execution.mirror_blob(digest).await?));
    }
    if raw.is_empty() {
        return Ok(None);
    }
    Ok(Some(cache.put_blob(raw).await?))
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

async fn forward(
    cache: &CacheProvider,
    workspace: &Path,
    digest: &once_cas::Digest,
    stream: Stream,
) -> Result<()> {
    let scratch = workspace.join(".once/tmp");
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|source| file_error("forward_remote_output", &scratch, source))?;
    let mut reader = cache.stage_blob(digest, &scratch).await?.into_reader()?;
    tokio::task::spawn_blocking(move || {
        match stream {
            Stream::Stdout => std::io::copy(&mut reader, &mut std::io::stdout().lock()),
            Stream::Stderr => std::io::copy(&mut reader, &mut std::io::stderr().lock()),
        }
        .and_then(|_| match stream {
            Stream::Stdout => std::io::stdout().flush(),
            Stream::Stderr => std::io::stderr().flush(),
        })
    })
    .await
    .map_err(|source| tree::api_error(format!("output forwarding task failed: {source}")))?
    .map_err(|source| file_error("forward_remote_output", Path::new(""), source))
}

enum Entry {
    Directory(PathBuf),
    File(PathBuf, reapi::Digest, bool),
    Link(PathBuf, String),
}

/// Every entry the result's outputs contain, refusing undeclared paths and
/// reporting a declared output the worker did not produce.
async fn output_entries(
    execution: &TuistExecution,
    cache: &CacheProvider,
    command: &PreparedCommand<'_>,
    cwd: &str,
    result: &reapi::ActionResult,
    limits: RemoteIoLimits,
) -> Result<Vec<Entry>> {
    let at = |path: &str| -> Result<PathBuf> {
        let joined = if cwd.is_empty() {
            path.to_string()
        } else {
            format!("{cwd}/{path}")
        };
        let joined = WorkspacePath::try_from(joined.as_str())?;
        if !command
            .outputs
            .iter()
            .any(|output| Path::new(joined.as_str()).starts_with(output.as_str()))
        {
            return Err(tree::api_error(format!(
                "the worker returned undeclared output `{}`",
                joined.as_str()
            )));
        }
        Ok(PathBuf::from(joined.as_str()))
    };
    let produced = result
        .output_files
        .iter()
        .map(|file| &file.path)
        .chain(
            result
                .output_directories
                .iter()
                .map(|directory| &directory.path),
        )
        .chain(result.output_symlinks.iter().map(|link| &link.path))
        .map(|path| at(path))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    if let Some(missing) = command
        .outputs
        .iter()
        .find(|output| !produced.contains(Path::new(output.as_str())))
    {
        return Err(Error::MissingOutput {
            path: missing.as_str().to_string(),
        });
    }
    let mut entries = Vec::new();
    for file in &result.output_files {
        let digest = file
            .digest
            .clone()
            .ok_or_else(|| tree::api_error("output file without a digest".into()))?;
        entries.push(Entry::File(at(&file.path)?, digest, file.is_executable));
    }
    for link in &result.output_symlinks {
        entries.push(Entry::Link(at(&link.path)?, link.target.clone()));
    }
    for directory in &result.output_directories {
        let digest = directory
            .tree_digest
            .as_ref()
            .ok_or_else(|| tree::api_error("output directory without a tree".into()))?;
        if u64::try_from(digest.size_bytes).map_or(true, |size| size > MAX_OUTPUT_TREE_BYTES) {
            return Err(tree::api_error("output tree exceeds 4 MiB".into()));
        }
        let local = execution.mirror_blob(digest).await?;
        let (bytes, truncated) = cache
            .read_blob_limited(&local, MAX_OUTPUT_TREE_BYTES, false)
            .await?;
        if truncated {
            return Err(tree::api_error("output tree exceeds 4 MiB".into()));
        }
        let output_tree = reapi::Tree::decode(bytes.as_slice())
            .map_err(|_| tree::api_error("malformed output tree".into()))?;
        tree_entries(&output_tree, at(&directory.path)?, &mut entries, limits)?;
    }
    Ok(entries)
}

async fn install_outputs(
    execution: &TuistExecution,
    cache: &CacheProvider,
    workspace: &Path,
    command: &PreparedCommand<'_>,
    cwd: &str,
    result: &reapi::ActionResult,
    limits: RemoteIoLimits,
) -> Result<()> {
    let entries = output_entries(execution, cache, command, cwd, result, limits).await?;
    // Digests carry every size, so an oversized result is refused before any
    // of it is downloaded.
    let bytes = entries
        .iter()
        .map(|entry| match entry {
            Entry::File(_, digest, _) => u64::try_from(digest.size_bytes).unwrap_or(u64::MAX),
            Entry::Directory(_) | Entry::Link(..) => 0,
        })
        .fold(0_u64, u64::saturating_add);
    if bytes > limits.max_archive_bytes {
        return Err(Error::ExecutorResourceLimit {
            executor: PROVIDER.into(),
            resource: "output bytes",
            limit: limits.max_archive_bytes,
        });
    }
    // Download every file into the local store before reserving staging, so
    // the reservation is not held across the network.
    let entries = futures::stream::iter(entries)
        .map(|entry| {
            async move {
                let local = match &entry {
                    Entry::File(_, digest, _) => Some(execution.mirror_blob(digest).await?),
                    Entry::Directory(_) | Entry::Link(..) => None,
                };
                Ok::<_, Error>((entry, local))
            }
            .boxed()
        })
        .buffer_unordered(PARALLEL_TRANSFERS)
        .try_collect::<Vec<_>>()
        .await?;
    // The staged copy and the backup of any outputs it replaces.
    let phase = super::phase_permit(command.lease.as_ref(), bytes.saturating_mul(2)).await?;
    let staging = OutputStaging::create_limited(workspace, limits)?;
    let files = staging.files_directory().clone();
    let scratch = workspace.join(".once/tmp");
    futures::stream::iter(entries)
        .map(|(entry, local)| {
            let (files, scratch) = (files.clone(), scratch.clone());
            async move { write_entry(cache, files, &scratch, entry, local).await }.boxed()
        })
        .buffer_unordered(PARALLEL_TRANSFERS)
        .try_collect::<()>()
        .await?;
    staging
        .install_admitted(command.outputs, workspace, phase)
        .await
}

fn tree_entries(
    tree: &reapi::Tree,
    base: PathBuf,
    entries: &mut Vec<Entry>,
    limits: RemoteIoLimits,
) -> Result<()> {
    let children = tree
        .children
        .iter()
        .map(|child| {
            (
                tree::key(&tree::digest_bytes(&child.encode_to_vec())),
                child,
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let root = tree
        .root
        .as_ref()
        .ok_or_else(|| tree::api_error("output tree without a root".into()))?;
    let mut pending = vec![(base, root)];
    while let Some((path, directory)) = pending.pop() {
        if entries.len() >= limits.max_archive_entries {
            return Err(Error::ExecutorResourceLimit {
                executor: PROVIDER.into(),
                resource: "output entries",
                limit: limits.max_archive_entries as u64,
            });
        }
        entries.push(Entry::Directory(path.clone()));
        for file in &directory.files {
            let digest = file
                .digest
                .clone()
                .ok_or_else(|| tree::api_error("output file without a digest".into()))?;
            entries.push(Entry::File(
                path.join(name(&file.name)?),
                digest,
                file.is_executable,
            ));
        }
        for link in &directory.symlinks {
            entries.push(Entry::Link(
                path.join(name(&link.name)?),
                link.target.clone(),
            ));
        }
        for child in &directory.directories {
            let digest = child
                .digest
                .as_ref()
                .ok_or_else(|| tree::api_error("output directory without a digest".into()))?;
            let child_directory = children.get(&tree::key(digest)).ok_or_else(|| {
                tree::api_error("output tree omits a referenced directory".into())
            })?;
            pending.push((path.join(name(&child.name)?), child_directory));
        }
    }
    Ok(())
}

fn name(value: &str) -> Result<&str> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\0']) {
        return Err(tree::api_error(format!(
            "invalid output entry name `{value}`"
        )));
    }
    Ok(value)
}

async fn write_entry(
    cache: &CacheProvider,
    files: Directory,
    scratch: &Path,
    entry: Entry,
    local: Option<once_cas::Digest>,
) -> Result<()> {
    let reader = match &local {
        Some(local) => Some(cache.stage_blob(local, scratch).await?.into_reader()?),
        None => None,
    };
    tokio::task::spawn_blocking(move || {
        let (path, written) = match entry {
            Entry::Directory(path) => {
                let written = files.descend(&path, true).map(drop);
                (path, written)
            }
            Entry::Link(path, target) => {
                let written = files.symlink(&path, Path::new(&target));
                (path, written)
            }
            Entry::File(path, _, executable) => {
                let mut reader = reader.expect("files are staged");
                let written = files
                    .create_file(&path, if executable { 0o755 } else { 0o644 })
                    .and_then(|mut file| std::io::copy(&mut reader, &mut file).map(drop));
                (path, written)
            }
        };
        written.map_err(|source| file_error("stage_remote_output", &path, source))
    })
    .await
    .map_err(|source| tree::api_error(format!("output staging task failed: {source}")))?
}

fn file_error(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_are_relative_to_the_working_directory_and_cannot_leave_it() {
        assert_eq!(relative_to("", "out/lib.rlib").unwrap(), "out/lib.rlib");
        assert_eq!(relative_to("crates/a", "crates/a/out").unwrap(), "out");
        assert!(relative_to("crates/a", "crates/ab/out").is_err());
        assert!(relative_to("crates/a", "crates/a").is_err());
        assert!(relative_to("crates/a", ".once/out/a").is_err());
    }

    #[tokio::test]
    async fn uploads_refuse_inputs_that_changed_or_moved_behind_a_link() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("src")).unwrap();
        std::fs::write(workspace.path().join("src/lib.rs"), b"pub fn a() {}").unwrap();
        std::fs::write(outside.path().join("lib.rs"), b"pub fn a() {}").unwrap();
        let digest = tree::digest_bytes(b"pub fn a() {}");
        let relative = PathBuf::from("src/lib.rs");
        let copy = verified_copy(workspace.path(), relative.clone(), &digest)
            .await
            .unwrap();
        assert_eq!(std::fs::read(copy.path()).unwrap(), b"pub fn a() {}");

        std::fs::write(workspace.path().join("src/lib.rs"), b"pub fn b() {}").unwrap();
        assert!(verified_copy(workspace.path(), relative.clone(), &digest)
            .await
            .is_err());

        std::fs::remove_dir_all(workspace.path().join("src")).unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("src")).unwrap();
        assert!(verified_copy(workspace.path(), relative, &digest)
            .await
            .is_err());
    }
}
