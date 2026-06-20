use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};
use sha2::Digest as ShaDigest;

use crate::{local, outputs, remote, Action, Error, OutputSymlinkMode, Result, WorkspacePath};

pub(crate) async fn run(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            input_digest: _,
            outputs,
            output_symlink_mode,
            resources: _,
            timeout_ms,
            remote,
        } => {
            let mut result = match (remote, stream_to_parent) {
                (Some(remote), _) => {
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
                    .await?
                }
                (None, true) => {
                    local::execute_command_streaming(
                        argv,
                        env,
                        cwd.as_ref(),
                        *timeout_ms,
                        workspace_root,
                        cache,
                    )
                    .await?
                }
                (None, false) => {
                    local::execute_command(
                        argv,
                        env,
                        cwd.as_ref(),
                        *timeout_ms,
                        workspace_root,
                        cache,
                    )
                    .await?
                }
            };
            if result.exit_code == 0 {
                result.outputs =
                    outputs::capture(outputs, workspace_root, cache, *output_symlink_mode).await?;
            }
            Ok(result)
        }
        Action::WriteFile { path, content, .. } => {
            write_file(path, content.as_bytes(), workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(path), workspace_root, cache).await
        }
        Action::WriteBytes { path, bytes, .. } => {
            write_file(path, bytes, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(path), workspace_root, cache).await
        }
        Action::CopyFile {
            source,
            destination,
            ..
        } => {
            copy_file(source, destination, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(destination), workspace_root, cache)
                .await
        }
        Action::CopyTree {
            sources,
            destination,
            ..
        } => {
            copy_tree(sources, destination, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(destination), workspace_root, cache)
                .await
        }
        Action::RemovePath { path, .. } => {
            remove_path(path, workspace_root).await?;
            Ok(empty_file_action_result())
        }
        Action::EnsureDir { path, .. } => {
            ensure_dir(path, workspace_root).await?;
            capture_file_action_outputs(std::slice::from_ref(path), workspace_root, cache).await
        }
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
    let metadata = std::fs::metadata(source)?;
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
        let metadata = std::fs::metadata(&child_path)?;
        if metadata.is_dir() {
            copy_directory_contents_blocking(&child_path, &child_destination)?;
        } else if metadata.is_file() {
            copy_file_blocking(&child_path, &child_destination)?;
        }
    }
    Ok(())
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
    let mut buf = [0u8; 64 * 1024];
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
