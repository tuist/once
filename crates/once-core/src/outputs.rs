use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider, Digest};

use crate::directory_blob::{capture_directory_blob, restore_directory_blob, DIRECTORY_BLOB_MAGIC};
use crate::file_blob::{capture_file_blob, restore_file_blob, FILE_BLOB_MAGIC};
use crate::{Error, Result, WorkspacePath};

/// Materialize every cached output blob to its declared workspace path.
/// On cache hit this is what makes a downstream action see a file the
/// upstream action did not actually run on this machine.
pub(crate) async fn restore(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    for (rel, digest) in &result.outputs {
        let abs = workspace_root.join(rel);
        let bytes = cache.get_blob(digest).await?;
        if bytes.starts_with(DIRECTORY_BLOB_MAGIC) {
            restore_directory_blob(rel, &abs, &bytes)?;
            continue;
        }
        if bytes.starts_with(FILE_BLOB_MAGIC) {
            restore_file_blob(rel, &abs, &bytes)?;
            continue;
        }
        restore_legacy_file(rel, &abs, &bytes).await?;
    }
    Ok(())
}

async fn restore_legacy_file(rel: &str, abs: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Error::RestoreOutput {
                path: rel.to_string(),
                source,
            })?;
    }
    let mut file = tokio::fs::File::create(&abs)
        .await
        .map_err(|source| Error::RestoreOutput {
            path: rel.to_string(),
            source,
        })?;
    file.write_all(bytes)
        .await
        .map_err(|source| Error::RestoreOutput {
            path: rel.to_string(),
            source,
        })?;
    file.flush().await.map_err(|source| Error::RestoreOutput {
        path: rel.to_string(),
        source,
    })?;
    Ok(())
}

/// Hash and store every declared output in the CAS, returning the
/// (path -> digest) map that goes into the cached `ActionResult`.
pub(crate) async fn capture(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
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
        let bytes = if metadata.is_dir() {
            capture_directory_blob(&abs).map_err(|source| Error::ReadOutput {
                path: rel.as_str().to_string(),
                source,
            })?
        } else {
            capture_file_blob(&abs).map_err(|source| Error::ReadOutput {
                path: rel.as_str().to_string(),
                source,
            })?
        };
        let digest = cache.put_blob(&bytes).await?;
        captured.insert(rel.as_str().to_string(), digest);
    }
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cas::Cas;
    use tempfile::TempDir;

    fn workspace_and_cache() -> (TempDir, std::path::PathBuf, CacheProvider) {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let cas_root = tmp.path().join("cas");
        std::fs::create_dir(&workspace).unwrap();
        (tmp, workspace, CacheProvider::Local(Cas::open(cas_root)))
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
        let outputs = capture(std::slice::from_ref(&output), &workspace, &cache)
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
}
