use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Error, Result, WorkspacePath};

pub(super) struct OutputStaging {
    root: PathBuf,
    files: PathBuf,
    backup: PathBuf,
}

impl OutputStaging {
    pub(super) fn create(workspace_root: &Path) -> Result<Self> {
        let parent = workspace_root.join(".once/tmp");
        std::fs::create_dir_all(&parent).map_err(|source| {
            host_error(
                "create_remote_output_staging",
                Path::new(".once/tmp"),
                source,
            )
        })?;
        for attempt in 0..100 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let root = parent.join(format!(
                "remote-output-{}-{nanos}-{attempt}",
                std::process::id()
            ));
            match std::fs::create_dir(&root) {
                Ok(()) => {
                    let files = root.join("files");
                    let backup = root.join("backup");
                    std::fs::create_dir(&files).map_err(|source| {
                        host_error("create_remote_output_staging", Path::new("files"), source)
                    })?;
                    std::fs::create_dir(&backup).map_err(|source| {
                        host_error("create_remote_output_staging", Path::new("backup"), source)
                    })?;
                    return Ok(Self {
                        root,
                        files,
                        backup,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(host_error(
                        "create_remote_output_staging",
                        Path::new(".once/tmp"),
                        source,
                    ));
                }
            }
        }
        Err(host_error(
            "create_remote_output_staging",
            Path::new(".once/tmp"),
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a unique remote output staging directory",
            ),
        ))
    }

    pub(super) fn files(&self) -> &Path {
        &self.files
    }

    pub(super) async fn install(
        self,
        outputs: &[WorkspacePath],
        workspace_root: &Path,
    ) -> Result<()> {
        let files = self.files.clone();
        let backup = self.backup.clone();
        let workspace = workspace_root.to_path_buf();
        let outputs = outputs.to_vec();
        tokio::task::spawn_blocking(move || install_outputs(&files, &backup, &workspace, &outputs))
            .await
            .map_err(|source| {
                host_error(
                    "install_remote_outputs",
                    Path::new(".once/tmp"),
                    std::io::Error::other(source.to_string()),
                )
            })?
    }
}

impl Drop for OutputStaging {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %self.root.display(), %error, "failed to remove remote output staging directory");
            }
        }
    }
}

fn install_outputs(
    files: &Path,
    backup: &Path,
    workspace: &Path,
    outputs: &[WorkspacePath],
) -> Result<()> {
    let mut backed_up = Vec::new();
    let mut installed = Vec::new();
    for output in outputs {
        if let Err(error) = install_output(
            files,
            backup,
            workspace,
            output,
            &mut backed_up,
            &mut installed,
        ) {
            rollback_outputs(&installed, &backed_up);
            return Err(error);
        }
    }
    Ok(())
}

fn install_output(
    files: &Path,
    backup: &Path,
    workspace: &Path,
    output: &WorkspacePath,
    backed_up: &mut Vec<(PathBuf, PathBuf)>,
    installed: &mut Vec<PathBuf>,
) -> Result<()> {
    let destination = output.resolve(workspace);
    if std::fs::symlink_metadata(&destination).is_ok() {
        let saved = output.resolve(backup);
        create_parent(&saved)?;
        std::fs::rename(&destination, &saved).map_err(|source| {
            host_error("backup_remote_output", Path::new(output.as_str()), source)
        })?;
        backed_up.push((destination.clone(), saved));
    }
    let source = output.resolve(files);
    create_parent(&destination)?;
    std::fs::rename(&source, &destination).map_err(|source| {
        host_error("install_remote_output", Path::new(output.as_str()), source)
    })?;
    installed.push(destination);
    Ok(())
}

fn rollback_outputs(installed: &[PathBuf], backed_up: &[(PathBuf, PathBuf)]) {
    for path in installed.iter().rev() {
        let _ = remove_host_path(path);
    }
    for (destination, saved) in backed_up.iter().rev() {
        let _ = std::fs::rename(saved, destination);
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| host_error("create_remote_output_parent", path, source))?;
    }
    Ok(())
}

fn remove_host_path(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn host_error(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: path.display().to_string(),
        source,
    }
}
