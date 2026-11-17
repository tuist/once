use std::io::{Read, Seek};
use std::path::Path;
use std::sync::Arc;

use crate::directory_blob::{is_directory_blob, prepare_directory, Preparation};
use crate::{Error, ResourcePermit, Result};

use super::{PrefetchedOutput, StagingDir};

pub(super) async fn restore(
    output: PrefetchedOutput,
    workspace: &Path,
    staging: Arc<StagingDir>,
    permit: Arc<ResourcePermit>,
) -> Result<()> {
    let destination = workspace.join(&output.rel);
    let mut input = output.blob.into_reader()?;
    let mut prefix = [0_u8; 32];
    let mut count = 0;
    while count < prefix.len() {
        let read = input
            .read(&mut prefix[count..])
            .map_err(|source| Error::RestoreOutput {
                path: output.rel.clone(),
                source,
            })?;
        if read == 0 {
            break;
        }
        count += read;
    }
    input.rewind().map_err(|source| Error::RestoreOutput {
        path: output.rel.clone(),
        source,
    })?;
    let prepared = if is_directory_blob(&prefix[..count]) {
        prepare_directory(&output.rel, &destination, input, (staging, permit)).await?
    } else {
        Preparation::Unavailable(input)
    };
    super::commit::run(|| {
        super::with_output_lock(workspace, || {
            if super::existing_output_matches_digest(&destination, output.digest) {
                return Ok(());
            }
            match prepared {
                Preparation::Ready(tree) => tree.publish(&output.rel, &destination),
                Preparation::Unavailable(input) => {
                    super::restore_prefetched_output(&output.rel, &destination, input, false)
                }
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Action, CopyPathMode, ResourceLimits, ResourcePool, ResourceRequest, WorkspacePath,
    };
    use struo_cas::CacheProvider;

    #[tokio::test]
    async fn scoped_existing_roots_and_multi_outputs_keep_ordered_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(temp.path().join("cas"));
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(source.join("input")).unwrap();
        std::fs::write(source.join("input/child"), b"parent bytes").unwrap();
        std::fs::write(source.join("input/sibling"), b"sibling").unwrap();
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("input").unwrap()],
            destination: WorkspacePath::try_from("tree").unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: None,
        };
        let parent = crate::run_uncached(&action, &source, &cache, false)
            .await
            .unwrap();
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            1,
            1024 * 1024 * 1024,
        )));
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(workspace.join("tree")).unwrap();
        std::fs::write(workspace.join("tree/old"), b"old").unwrap();
        let permit = pool.acquire(ResourceRequest::default()).await;
        crate::with_resource_permit(permit, super::super::restore(&parent, &workspace, &cache))
            .await
            .unwrap();
        assert!(!workspace.join("tree/old").exists());
        assert_eq!(
            std::fs::read(workspace.join("tree/child")).unwrap(),
            b"parent bytes"
        );
        std::fs::remove_dir_all(workspace.join("tree")).unwrap();
        let mut multiple = parent;
        multiple.outputs.insert(
            "tree/child".to_string(),
            cache.put_blob(b"child bytes").await.unwrap(),
        );
        let permit = pool.acquire(ResourceRequest::default()).await;
        crate::with_resource_permit(permit, super::super::restore(&multiple, &workspace, &cache))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(workspace.join("tree/child")).unwrap(),
            b"child bytes"
        );
        assert_eq!(
            std::fs::read(workspace.join("tree/sibling")).unwrap(),
            b"sibling"
        );
    }
}
