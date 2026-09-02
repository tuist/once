use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use futures::StreamExt;
use once_cas::{ActionResult, CacheProvider};
use sha2::Digest as ShaDigest;
use tokio::io::AsyncWriteExt;

use crate::stream::ActionOutputObserver;
use crate::{
    archive, contract, local, outputs, remote, Action, CopyPathMode, Error, OutputSymlinkMode,
    PreparePathMode, Result, SandboxMode, WorkspacePath,
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
    if let Action::RunCommand {
        outputs,
        output_symlink_mode,
        ..
    } = action
    {
        let mut result = Box::pin(execute_command(
            action,
            workspace_root,
            cache,
            stream_to_parent,
            validate_contract,
            observer,
        ))
        .await?;
        let completed = action.accepts_exit_code(result.exit_code);
        if completed && !validate_contract {
            result.outputs =
                outputs::capture(outputs, workspace_root, cache, *output_symlink_mode).await?;
        }
        if completed {
            result.exit_code = 0;
        }
        return Ok(result);
    }

    execute_portable_action(action, workspace_root, cache).await
}

#[allow(clippy::too_many_lines)]
async fn execute_portable_action(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    match action {
        Action::WriteFile { path, bytes, .. } => {
            write_file(path, bytes, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(path), workspace_root, cache).await
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
                    copy_file(&sources[0], destination, workspace_root).await?;
                }
                CopyPathMode::Tree => {
                    copy_tree(sources, destination, workspace_root).await?;
                }
            }
            capture_file_action_outputs(std::slice::from_ref(destination), workspace_root, cache)
                .await
        }
        Action::LinkPath {
            source,
            destination,
            ..
        } => {
            link_path(source, destination, workspace_root).await?;
            Ok(empty_file_action_result())
        }
        Action::MaterializeHostFile {
            source,
            source_sha256,
            destination,
            ..
        } => {
            materialize_host_file(source, source_sha256, destination, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(destination), workspace_root, cache)
                .await
        }
        Action::MaterializeHostTree {
            source,
            source_sha256,
            destination,
            ..
        } => {
            materialize_host_tree(source, source_sha256, destination, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(destination), workspace_root, cache)
                .await
        }
        Action::PreparePath { path, mode, .. } => match mode {
            PreparePathMode::Remove => {
                remove_path(path, workspace_root).await?;
                Ok(empty_file_action_result())
            }
            PreparePathMode::Directory => {
                ensure_dir(path, workspace_root).await?;
                capture_file_action_outputs(std::slice::from_ref(path), workspace_root, cache).await
            }
        },
        Action::WriteTreeDigest {
            root,
            output,
            include_suffixes,
            ..
        } => {
            write_tree_digest(root, output, include_suffixes, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(output), workspace_root, cache).await
        }
        Action::WriteArchive {
            entries,
            output,
            sha256_output,
            format,
            ..
        } => {
            archive::write(
                entries,
                output,
                sha256_output.as_ref(),
                *format,
                workspace_root,
            )
            .await?;
            let mut outputs = vec![output.clone()];
            if let Some(path) = sha256_output {
                outputs.push(path.clone());
            }
            capture_file_action_outputs(&outputs, workspace_root, cache).await
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
            )
            .await?;
            capture_file_action_outputs(std::slice::from_ref(destination), workspace_root, cache)
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
) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|source| Error::InvalidDownloadAndExtract {
        reason: format!("invalid URL `{url}`: {source}"),
    })?;
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
    tokio::fs::create_dir_all(destination_parent)
        .await
        .map_err(|source| Error::FileAction {
            action: "download_and_extract",
            path: destination.as_str().to_string(),
            source,
        })?;

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
    let response = request
        .send()
        .await
        .map_err(|source| Error::InvalidDownloadAndExtract {
            reason: format!("requesting `{url}`: {source}"),
        })?
        .error_for_status()
        .map_err(|source| Error::InvalidDownloadAndExtract {
            reason: format!("requesting `{url}`: {source}"),
        })?;

    let archive = tempfile::NamedTempFile::new_in(destination_parent).map_err(|source| {
        Error::FileAction {
            action: "download_and_extract",
            path: destination.as_str().to_string(),
            source,
        }
    })?;
    let archive_path = archive.path().to_path_buf();
    let mut file = tokio::fs::File::create(&archive_path)
        .await
        .map_err(|source| Error::FileAction {
            action: "download_and_extract",
            path: archive_path.display().to_string(),
            source,
        })?;
    let mut digest = sha2::Sha256::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| Error::InvalidDownloadAndExtract {
            reason: format!("reading `{url}`: {source}"),
        })?;
        digest.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: archive_path.display().to_string(),
                source,
            })?;
    }
    file.flush().await.map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: archive_path.display().to_string(),
        source,
    })?;
    drop(file);

    let digest = digest.finalize();
    let mut actual_sha256 = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual_sha256, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(Error::DownloadChecksumMismatch {
            url: url.to_string(),
            expected: expected_sha256.to_string(),
            actual: actual_sha256,
        });
    }

    let extracted_destination = absolute_destination.clone();
    let staging_parent = destination_parent.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let staging = tempfile::Builder::new()
            .prefix(".once-download-")
            .tempdir_in(&staging_parent)
            .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: staging_parent.display().to_string(),
                source,
            })?;
        let staged_output = staging.path().join("output");
        extract_zip_archive(&archive_path, &staged_output)?;
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn extract_zip_archive(archive_path: &Path, destination: &Path) -> Result<()> {
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
    let mut archive =
        zip::ZipArchive::new(archive_file).map_err(|source| Error::InvalidDownloadAndExtract {
            reason: format!("opening ZIP archive: {source}"),
        })?;
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
        let output = destination.join(relative);
        let unix_mode = entry.unix_mode().unwrap_or(0);
        let file_type = unix_mode & 0o170_000;
        if file_type != 0 && file_type != 0o100_000 && file_type != 0o040_000 {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{}` has an unsupported file type", entry.name()),
            });
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
            continue;
        }
        let parent = output
            .parent()
            .ok_or_else(|| Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{}` has no parent directory", entry.name()),
            })?;
        std::fs::create_dir_all(parent).map_err(|source| Error::FileAction {
            action: "download_and_extract",
            path: parent.display().to_string(),
            source,
        })?;
        let mut output_file =
            std::fs::File::create(&output).map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
        std::io::copy(&mut entry, &mut output_file).map_err(|source| Error::FileAction {
            action: "download_and_extract",
            path: output.display().to_string(),
            source,
        })?;
        #[cfg(unix)]
        if unix_mode != 0 {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(unix_mode & 0o777))
                .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
        }
    }
    Ok(())
}

async fn execute_command(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
    observer: Option<&dyn ActionOutputObserver>,
) -> Result<ActionResult> {
    let Action::RunCommand {
        argv,
        env,
        cwd,
        inputs,
        outputs,
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

    match (remote, stream_to_parent, sandbox) {
        (Some(remote), _, _) => {
            remote::execute_command(
                remote,
                remote::PreparedCommand {
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
            )
            .await
        }
        (None, _, SandboxMode::Inputs | SandboxMode::CopiedInputs) => {
            execute_sandboxed_command(
                action,
                invocation,
                SandboxPolicy {
                    inputs,
                    outputs,
                    copy_inputs: *sandbox == SandboxMode::CopiedInputs,
                    validate_contract,
                },
                workspace_root,
                cache,
                stream_to_parent,
                observer,
            )
            .await
        }
        (None, _, SandboxMode::Off) => match observer {
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
    }
}

/// The filesystem contract a sandboxed run is held to.
///
/// Separate from the command itself because these describe the policy
/// around the run rather than what is executed, and because three bare
/// `bool`s in a positional argument list were unreadable at the call
/// site.
struct SandboxPolicy<'a> {
    /// Paths the action declared it reads.
    inputs: &'a [WorkspacePath],
    /// Paths the action declared it writes.
    outputs: &'a [WorkspacePath],
    /// Copy inputs into the execroot instead of linking them.
    copy_inputs: bool,
    /// Audit the execroot against the declaration once the child exits.
    validate_contract: bool,
}

async fn execute_sandboxed_command(
    action: &Action,
    invocation: local::Invocation<'_>,
    policy: SandboxPolicy<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    observer: Option<&dyn ActionOutputObserver>,
) -> Result<ActionResult> {
    let sandbox = prepare_input_sandbox(action, &policy, invocation.cwd, workspace_root).await?;
    let result = if let Some(observer) = observer {
        local::execute_command_observed(
            invocation,
            &sandbox.execroot,
            cache,
            stream_to_parent,
            observer,
        )
        .await
    } else if stream_to_parent {
        local::execute_command_streaming(invocation, &sandbox.execroot, cache).await
    } else {
        local::execute_command(invocation, &sandbox.execroot, cache).await
    };

    let result = result?;
    if policy.validate_contract {
        let violations = contract::audit_filesystem(
            &sandbox.execroot,
            workspace_root,
            policy.inputs,
            policy.outputs,
            &sandbox.before_execroot,
            &sandbox.before_workspace,
            &result,
            cache,
        )
        .await
        .map_err(|source| Error::FileAction {
            action: "audit_sandbox",
            path: sandbox.execroot.display().to_string(),
            source,
        })?;
        if !violations.is_empty() {
            return Err(Error::ContractViolation { violations });
        }
        return Ok(result);
    }
    if action.accepts_exit_code(result.exit_code) {
        copy_sandbox_outputs(policy.outputs, &sandbox.execroot, workspace_root).await?;
    }
    Ok(result)
}

struct PreparedSandbox {
    root: std::path::PathBuf,
    execroot: std::path::PathBuf,
    keep: bool,
    before_execroot: contract::ContractSnapshot,
    before_workspace: contract::ContractSnapshot,
}

impl Drop for PreparedSandbox {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    sandbox = %self.root.display(),
                    %error,
                    "failed to remove action sandbox"
                );
            }
        }
    }
}

async fn prepare_input_sandbox(
    action: &Action,
    policy: &SandboxPolicy<'_>,
    cwd: Option<&WorkspacePath>,
    workspace_root: &Path,
) -> Result<PreparedSandbox> {
    let SandboxPolicy {
        inputs,
        outputs,
        copy_inputs,
        validate_contract,
    } = *policy;
    let root = workspace_root
        .join(".once")
        .join("sandboxes")
        .join(action.digest().to_string());
    let execroot = root.join("execroot");
    let sandbox_root = root.clone();
    let sandbox_execroot = execroot.clone();
    let input_paths = inputs.to_vec();
    let output_paths = outputs.to_vec();
    let cwd_path = cwd.cloned();
    let workspace = workspace_root.to_path_buf();
    // Contract validation stages copies rather than symlinks so a probe that
    // writes one of its declared inputs mutates the private execroot copy (where
    // the audit still flags the write) instead of reaching through a symlink and
    // corrupting the real workspace source the docs promise to leave untouched.
    let copy_inputs = validate_contract || copy_inputs;
    tokio::task::spawn_blocking(move || {
        remove_path_blocking(&sandbox_root)?;
        std::fs::create_dir_all(&sandbox_execroot)?;
        for input in input_paths {
            stage_sandbox_input_blocking(&workspace, &sandbox_execroot, &input, copy_inputs)?;
        }
        for output in output_paths {
            if let Some(parent) = output.resolve(&sandbox_execroot).parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        if let Some(cwd) = cwd_path {
            std::fs::create_dir_all(cwd.resolve(&sandbox_execroot))?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "prepare_sandbox",
        path: root.display().to_string(),
        source: std::io::Error::other(source.to_string()),
    })?
    .map_err(|source| Error::FileAction {
        action: "prepare_sandbox",
        path: root.display().to_string(),
        source,
    })?;

    let before_execroot = if validate_contract {
        contract::snapshot_tree(&execroot, &[]).map_err(|source| Error::FileAction {
            action: "snapshot_sandbox",
            path: execroot.display().to_string(),
            source,
        })?
    } else {
        contract::ContractSnapshot(std::collections::BTreeMap::new())
    };
    let before_workspace = if validate_contract {
        contract::snapshot_tree(workspace_root, &[".once"]).map_err(|source| Error::FileAction {
            action: "snapshot_workspace",
            path: workspace_root.display().to_string(),
            source,
        })?
    } else {
        contract::ContractSnapshot(std::collections::BTreeMap::new())
    };
    Ok(PreparedSandbox {
        root,
        execroot,
        keep: keep_sandbox(),
        before_execroot,
        before_workspace,
    })
}

fn keep_sandbox() -> bool {
    std::env::var("ONCE_KEEP_SANDBOX")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
}

fn stage_sandbox_input_blocking(
    workspace_root: &Path,
    sandbox_execroot: &Path,
    input: &WorkspacePath,
    copy: bool,
) -> std::io::Result<()> {
    let source = input.resolve(workspace_root);
    let destination = input.resolve(sandbox_execroot);
    if copy {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        remove_path_blocking(&destination)?;
        return copy_sandbox_output_blocking(&source, &destination);
    }
    let metadata = std::fs::symlink_metadata(&source)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        stage_sandbox_directory_blocking(&source, &destination)
    } else {
        link_sandbox_input_blocking(&source, &destination)
    }
}

fn stage_sandbox_directory_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    let mut children = std::fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let child_source = child.path();
        let child_destination = destination.join(child.file_name());
        let metadata = std::fs::symlink_metadata(&child_source)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            stage_sandbox_directory_blocking(&child_source, &child_destination)?;
        } else {
            link_sandbox_input_blocking(&child_source, &child_destination)?;
        }
    }
    Ok(())
}

fn link_sandbox_input_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_blocking(destination)?;
    create_symlink_blocking(source, destination, source)
}

async fn copy_sandbox_outputs(
    outputs: &[WorkspacePath],
    sandbox_execroot: &Path,
    workspace_root: &Path,
) -> Result<()> {
    let output_paths = outputs.to_vec();
    let sandbox = sandbox_execroot.to_path_buf();
    let workspace = workspace_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        for output in output_paths {
            let source = output.resolve(&sandbox);
            if !source.try_exists()? {
                continue;
            }
            let destination = output.resolve(&workspace);
            copy_sandbox_output_blocking(&source, &destination)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "copy_sandbox_outputs",
        path: sandbox_execroot.display().to_string(),
        source: std::io::Error::other(source.to_string()),
    })?
    .map_err(|source| Error::FileAction {
        action: "copy_sandbox_outputs",
        path: sandbox_execroot.display().to_string(),
        source,
    })
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
) -> Result<ActionResult> {
    let mut result = empty_file_action_result();
    result.outputs = outputs::capture(
        action_outputs,
        workspace_root,
        cache,
        OutputSymlinkMode::default(),
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

async fn write_file(path: &WorkspacePath, bytes: &[u8], workspace_root: &Path) -> Result<()> {
    let absolute = path.resolve(workspace_root);
    if let Some(parent) = absolute.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Error::FileAction {
                action: "create_parent_dir",
                path: path.as_str().to_string(),
                source,
            })?;
    }
    tokio::fs::write(&absolute, bytes)
        .await
        .map_err(|source| Error::FileAction {
            action: "write_file",
            path: path.as_str().to_string(),
            source,
        })?;
    Ok(())
}

async fn copy_file(
    source: &WorkspacePath,
    destination: &WorkspacePath,
    workspace_root: &Path,
) -> Result<()> {
    let absolute_source = source.resolve(workspace_root);
    let absolute_destination = destination.resolve(workspace_root);
    let destination_label = destination.as_str().to_string();
    tokio::task::spawn_blocking(move || {
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
) -> Result<()> {
    if !source.is_absolute() {
        return Err(Error::InvalidHostFile {
            reason: format!("source `{}` must be absolute", source.display()),
        });
    }
    let bytes = tokio::fs::read(source)
        .await
        .map_err(|source_error| Error::FileAction {
            action: "read_host_file",
            path: source.display().to_string(),
            source: source_error,
        })?;
    let digest = sha2::Sha256::digest(&bytes);
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if actual != source_sha256 {
        return Err(Error::HostFileDigestMismatch {
            path: source.display().to_string(),
            expected: source_sha256.to_string(),
            actual,
        });
    }
    write_file(destination, &bytes, workspace_root).await
}

async fn materialize_host_tree(
    source: &Path,
    source_sha256: &str,
    destination: &WorkspacePath,
    workspace_root: &Path,
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
    tokio::task::spawn_blocking(move || {
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
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
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
) -> Result<()> {
    let source_paths = sources
        .iter()
        .map(|source| source.resolve(workspace_root))
        .collect::<Vec<_>>();
    let destination_path = destination.resolve(workspace_root);
    let destination_label = destination.as_str().to_string();
    tokio::task::spawn_blocking(move || {
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

async fn remove_path(path: &WorkspacePath, workspace_root: &Path) -> Result<()> {
    let absolute = path.resolve(workspace_root);
    remove_path_if_exists(&absolute, "remove_path", path.as_str()).await
}

async fn ensure_dir(path: &WorkspacePath, workspace_root: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path.resolve(workspace_root))
        .await
        .map_err(|source| Error::FileAction {
            action: "ensure_dir",
            path: path.as_str().to_string(),
            source,
        })
}

async fn link_path(
    source: &WorkspacePath,
    destination: &WorkspacePath,
    workspace_root: &Path,
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
    tokio::task::spawn_blocking(move || {
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
) -> Result<()> {
    let root_path = root.resolve(workspace_root);
    let output_path = output.resolve(workspace_root);
    let output_label = output.as_str().to_string();
    let suffixes = include_suffixes.to_vec();
    let bytes = tokio::task::spawn_blocking(move || tree_digest_bytes(&root_path, &suffixes))
        .await
        .map_err(|source| Error::FileAction {
            action: "write_tree_digest",
            path: output_label.clone(),
            source: std::io::Error::other(source.to_string()),
        })?
        .map_err(|source| Error::FileAction {
            action: "write_tree_digest",
            path: output_label.clone(),
            source,
        })?;
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Error::FileAction {
                action: "create_parent_dir",
                path: output.as_str().to_string(),
                source,
            })?;
    }
    tokio::fs::write(&output_path, bytes)
        .await
        .map_err(|source| Error::FileAction {
            action: "write_tree_digest",
            path: output.as_str().to_string(),
            source,
        })
}

async fn remove_path_if_exists(abs: &Path, action: &'static str, label: &str) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(abs).await {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::FileAction {
                action,
                path: label.to_string(),
                source,
            });
        }
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(abs).await
    } else {
        tokio::fs::remove_file(abs).await
    };
    result.map_err(|source| Error::FileAction {
        action,
        path: label.to_string(),
        source,
    })
}

fn remove_path_blocking(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(source),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
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
    use super::{copy_tree_contents_blocking, file_sha256_hex, hex_lower, tree_digest_bytes};

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
