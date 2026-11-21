use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use struo_cas::Digest;

pub(super) struct OutputLock {
    file: File,
    exclusive: bool,
}

impl OutputLock {
    pub(super) fn open_all<'a>(
        workspace: &Path,
        digest: Digest,
        outputs: impl IntoIterator<Item = &'a str>,
    ) -> Result<Vec<Self>> {
        let directory = workspace.join(".struo/locks/actions");
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("creating action lock directory `{}`", directory.display()))?;
        let mut paths = BTreeMap::<PathBuf, bool>::new();
        for output in outputs {
            let normalized = super::workspace_path(output, "action lock path")?;
            let path = PathBuf::from(normalized.as_str());
            paths.insert(path.clone(), true);
            // Shared ancestor locks let siblings proceed, but exclude writers
            // replacing a whole tree while one of its children is in use.
            for ancestor in path
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
            {
                paths.entry(ancestor.to_path_buf()).or_insert(false);
            }
        }
        let mut keys = paths
            .into_iter()
            .map(|(path, exclusive)| {
                (
                    Digest::of_bytes(path.as_os_str().as_encoded_bytes()),
                    exclusive,
                )
            })
            .collect::<Vec<_>>();
        if keys.is_empty() {
            keys.push((digest, true));
        }
        keys.into_iter()
            .map(|(key, exclusive)| {
                let path = directory.join(format!("{key}.lock"));
                let file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&path)
                    .with_context(|| format!("opening action lock `{}`", path.display()))?;
                Ok(Self { file, exclusive })
            })
            .collect()
    }

    pub(super) fn try_lock(&self) -> std::result::Result<(), TryLockError> {
        if self.exclusive {
            self.file.try_lock()
        } else {
            self.file.try_lock_shared()
        }
    }

    pub(super) fn unlock(&self) -> std::io::Result<()> {
        self.file.unlock()
    }
}
