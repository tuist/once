use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};
use sha2::Digest as ShaDigest;

use crate::{
    contract, local, outputs, remote, Action, CopyPathMode, Error, OutputSymlinkMode,
    PreparePathMode, Result, SandboxMode, WorkspacePath,
};

pub(crate) async fn run(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            outputs,
            output_symlink_mode,
            ..
        } => {
            let mut result = Box::pin(execute_command(
                action,
                workspace_root,
                cache,
                stream_to_parent,
                validate_contract,
            ))
            .await?;
            if result.exit_code == 0 && !validate_contract {
                result.outputs =
                    outputs::capture(outputs, workspace_root, cache, *output_symlink_mode).await?;
            }
            Ok(result)
        }
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
    }
}

async fn execute_command(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    validate_contract: bool,
) -> Result<ActionResult> {
    let Action::RunCommand {
        argv,
        env,
        cwd,
        inputs,
        outputs,
        timeout_ms,
        remote,
        stdout_path,
        stderr_path,
        sandbox,
        ..
    } = action
    else {
        unreachable!("execute_command only accepts command actions")
    };

    let redirect = local::Redirect {
        stdout: stdout_path.as_deref(),
        stderr: stderr_path.as_deref(),
    };

    match (remote, stream_to_parent, sandbox) {
        (Some(remote), _, _) => {
            remote::execute_command(
                remote,
                argv,
                env,
                cwd.as_ref(),
                *timeout_ms,
                workspace_root,
                cache,
                stream_to_parent,
            )
            .await
        }
        (None, _, SandboxMode::Inputs) => {
            execute_sandboxed_command(
                action,
                argv,
                env,
                cwd.as_ref(),
                inputs,
                outputs,
                *timeout_ms,
                workspace_root,
                cache,
                redirect,
                stream_to_parent,
                validate_contract,
            )
            .await
        }
        (None, true, SandboxMode::Off) => {
            local::execute_command_streaming(
                argv,
                env,
                cwd.as_ref(),
                *timeout_ms,
                workspace_root,
                cache,
                redirect,
            )
            .await
        }
        (None, false, SandboxMode::Off) => {
            Box::pin(local::execute_command(
                argv,
                env,
                cwd.as_ref(),
                *timeout_ms,
                workspace_root,
                cache,
                redirect,
            ))
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_sandboxed_command(
    action: &Action,
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    inputs: &[WorkspacePath],
    outputs: &[WorkspacePath],
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
    redirect: local::Redirect<'_>,
    stream_to_parent: bool,
    validate_contract: bool,
) -> Result<ActionResult> {
    let sandbox =
        prepare_input_sandbox(action, inputs, cwd, workspace_root, validate_contract).await?;
    let result = if stream_to_parent {
        local::execute_command_streaming(
            argv,
            env,
            cwd,
            timeout_ms,
            &sandbox.execroot,
            cache,
            redirect,
        )
        .await
    } else {
        local::execute_command(
            argv,
            env,
            cwd,
            timeout_ms,
            &sandbox.execroot,
            cache,
            redirect,
        )
        .await
    };

    match result {
        Ok(result) => {
            if validate_contract {
                let violations = contract::audit_filesystem(
                    &sandbox.execroot,
                    workspace_root,
                    inputs,
                    outputs,
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
            if result.exit_code == 0 {
                copy_sandbox_outputs(outputs, &sandbox.execroot, workspace_root).await?;
            }
            Ok(result)
        }
        Err(error) => Err(error),
    }
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
    inputs: &[WorkspacePath],
    cwd: Option<&WorkspacePath>,
    workspace_root: &Path,
    validate_contract: bool,
) -> Result<PreparedSandbox> {
    let root = workspace_root
        .join(".once")
        .join("sandboxes")
        .join(action.digest().to_string());
    let execroot = root.join("execroot");
    let sandbox_root = root.clone();
    let sandbox_execroot = execroot.clone();
    let input_paths = inputs.to_vec();
    let cwd_path = cwd.cloned();
    let workspace = workspace_root.to_path_buf();
    // Contract validation stages copies rather than symlinks so a probe that
    // writes one of its declared inputs mutates the private execroot copy (where
    // the audit still flags the write) instead of reaching through a symlink and
    // corrupting the real workspace source the docs promise to leave untouched.
    let copy_inputs = validate_contract;
    tokio::task::spawn_blocking(move || {
        remove_path_blocking(&sandbox_root)?;
        std::fs::create_dir_all(&sandbox_execroot)?;
        for input in input_paths {
            stage_sandbox_input_blocking(&workspace, &sandbox_execroot, &input, copy_inputs)?;
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
    let absolute_destination = destination.resolve(workspace_root);
    if let Some(parent) = absolute_destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Error::FileAction {
                action: "create_parent_dir",
                path: destination.as_str().to_string(),
                source,
            })?;
    }
    remove_path_if_exists(&absolute_destination, "copy_file", destination.as_str()).await?;
    tokio::fs::copy(source.resolve(workspace_root), &absolute_destination)
        .await
        .map_err(|source| Error::FileAction {
            action: "copy_file",
            path: destination.as_str().to_string(),
            source,
        })?;
    Ok(())
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
