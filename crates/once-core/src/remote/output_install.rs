use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::remote::path::{Directory, Parent};
use crate::remote::PhasePermit;
use crate::{Error, RemoteIoLimits, Result, WorkspacePath};

mod lock;
mod publish;
#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests;
#[cfg(all(test, not(any(target_os = "linux", target_vendor = "apple"))))]
mod unsupported;

pub(super) struct OutputStaging {
    root: PathBuf,
    #[cfg(any(
        feature = "remote-microsandbox",
        all(test, any(target_os = "linux", target_vendor = "apple"))
    ))]
    files: PathBuf,
    root_parent: Parent,
    files_dir: Directory,
    backup_dir: Directory,
    workspace_dir: Directory,
    limits: RemoteIoLimits,
    directory_modes: BTreeMap<PathBuf, u32>,
}

impl OutputStaging {
    #[cfg(any(
        feature = "remote-microsandbox",
        all(test, any(target_os = "linux", target_vendor = "apple"))
    ))]
    pub(super) fn create(workspace_root: &Path) -> Result<Self> {
        Self::create_limited(workspace_root, RemoteIoLimits::default())
    }

    pub(super) fn create_limited(workspace_root: &Path, limits: RemoteIoLimits) -> Result<Self> {
        let parent = workspace_root.join(".once/tmp");
        let workspace_dir = Directory::open(workspace_root)
            .map_err(|source| host_error("open_remote_output_workspace", workspace_root, source))?;
        let parent_dir = workspace_dir
            .descend(Path::new(".once/tmp"), true)
            .map_err(|source| {
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
            let name = PathBuf::from(format!(
                "remote-output-{}-{nanos}-{attempt}",
                std::process::id()
            ));
            let root = parent.join(&name);
            match parent_dir.mkdir(&name) {
                Ok(()) => {
                    #[cfg(any(
                        feature = "remote-microsandbox",
                        all(test, any(target_os = "linux", target_vendor = "apple"))
                    ))]
                    let files = root.join("files");
                    let root_dir = parent_dir.descend(&name, false).map_err(|source| {
                        host_error("create_remote_output_staging", &name, source)
                    })?;
                    let files_dir =
                        root_dir
                            .descend(Path::new("files"), true)
                            .map_err(|source| {
                                host_error(
                                    "create_remote_output_staging",
                                    Path::new("files"),
                                    source,
                                )
                            })?;
                    let backup_dir =
                        root_dir
                            .descend(Path::new("backup"), true)
                            .map_err(|source| {
                                host_error(
                                    "create_remote_output_staging",
                                    Path::new("backup"),
                                    source,
                                )
                            })?;
                    let root_parent = parent_dir.parent(&name, false).map_err(|source| {
                        host_error("create_remote_output_staging", &name, source)
                    })?;
                    return Ok(Self {
                        root,
                        #[cfg(any(
                            feature = "remote-microsandbox",
                            all(test, any(target_os = "linux", target_vendor = "apple"))
                        ))]
                        files,
                        root_parent,
                        files_dir,
                        backup_dir,
                        workspace_dir,
                        limits,
                        directory_modes: BTreeMap::new(),
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

    #[cfg(any(
        feature = "remote-microsandbox",
        all(test, any(target_os = "linux", target_vendor = "apple"))
    ))]
    pub(super) fn files(&self) -> &Path {
        &self.files
    }

    pub(super) fn files_directory(&self) -> &Directory {
        &self.files_dir
    }

    pub(super) fn set_directory_modes(&mut self, modes: BTreeMap<PathBuf, u32>) {
        self.directory_modes = modes;
    }

    #[cfg(any(
        feature = "remote-microsandbox",
        all(test, any(target_os = "linux", target_vendor = "apple"))
    ))]
    pub(super) async fn install(
        self,
        outputs: &[WorkspacePath],
        workspace_root: &Path,
    ) -> Result<()> {
        self.install_admitted(outputs, workspace_root, PhasePermit::default())
            .await
    }

    pub(super) async fn install_admitted(
        self,
        outputs: &[WorkspacePath],
        _workspace_root: &Path,
        lease: PhasePermit,
    ) -> Result<()> {
        let outputs = outputs.to_vec();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let staging = self;
            lock::with_output_lock(&staging.workspace_dir, || {
                publish::install_outputs(
                    &staging.files_dir,
                    &staging.backup_dir,
                    &staging.workspace_dir,
                    &outputs,
                    staging.limits,
                    &staging.directory_modes,
                )
            })
        })
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
        if let Err(error) = self.root_parent.remove() {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %self.root.display(), %error, "failed to remove remote output staging directory");
            }
        }
    }
}

fn host_error(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: path.display().to_string(),
        source,
    }
}
