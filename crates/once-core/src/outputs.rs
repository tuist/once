use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider, Digest};

use crate::directory_blob::{capture_directory_blob, restore_directory_blob, DIRECTORY_BLOB_MAGIC};
use crate::{Error, Result, WorkspacePath};

/// Materialize every cached output blob to its declared workspace path.
/// On cache hit this is what makes a downstream action see a file the
/// upstream action did not actually run on this machine.
pub(crate) async fn restore(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    for (rel, digest) in &result.outputs {
        let abs = workspace_root.join(rel);
        let bytes = cache.get_blob(digest).await?;
        if bytes.starts_with(DIRECTORY_BLOB_MAGIC) {
            restore_directory_blob(rel, &abs, &bytes)?;
            continue;
        }
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| Error::RestoreOutput {
                    path: rel.clone(),
                    source,
                })?;
        }
        let mut file =
            tokio::fs::File::create(&abs)
                .await
                .map_err(|source| Error::RestoreOutput {
                    path: rel.clone(),
                    source,
                })?;
        file.write_all(&bytes)
            .await
            .map_err(|source| Error::RestoreOutput {
                path: rel.clone(),
                source,
            })?;
        file.flush().await.map_err(|source| Error::RestoreOutput {
            path: rel.clone(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            tokio::fs::set_permissions(&abs, perms)
                .await
                .map_err(|source| Error::RestoreOutput {
                    path: rel.clone(),
                    source,
                })?;
        }
    }
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
            tokio::fs::read(&abs)
                .await
                .map_err(|source| Error::ReadOutput {
                    path: rel.as_str().to_string(),
                    source,
                })?
        };
        let digest = cache.put_blob(&bytes).await?;
        captured.insert(rel.as_str().to_string(), digest);
    }
    Ok(captured)
}
