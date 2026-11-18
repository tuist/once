use super::*;
use crate::{with_resource_permit, ResourceLimits, ResourcePool, ResourceRequest};
use std::sync::Arc;

#[tokio::test]
async fn admitted_direct_capture_preserves_staged_directory_and_file_identity() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let tree = workspace.join("tree");
    std::fs::create_dir_all(tree.join("nested")).unwrap();
    std::fs::write(tree.join("nested/data"), vec![17; 512 * 1024 + 3]).unwrap();
    std::fs::write(workspace.join("file"), b"standalone").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            tree.join("nested/data"),
            std::fs::Permissions::from_mode(0o751),
        )
        .unwrap();
        std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o750)).unwrap();
        std::os::unix::fs::symlink("nested/data", tree.join("link")).unwrap();
        std::os::unix::fs::symlink(workspace.join("file"), tree.join("external")).unwrap();
    }
    let paths = [
        WorkspacePath::try_from("tree").unwrap(),
        WorkspacePath::try_from("file").unwrap(),
    ];
    for cpu_slots in [1, 2] {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            cpu_slots,
            1024 * 1024 * 1024,
        )));
        for mode in [
            OutputSymlinkMode::Preserve,
            OutputSymlinkMode::MaterializeExternal,
        ] {
            let staged =
                CacheProvider::open_local(root.path().join(format!("staged-{cpu_slots}-{mode:?}")));
            let direct =
                CacheProvider::open_local(root.path().join(format!("direct-{cpu_slots}-{mode:?}")));
            let expected = capture(&paths, &workspace, &staged, mode).await.unwrap();
            let permit = pool.acquire(ResourceRequest::default()).await;
            let actual = with_resource_permit(permit, capture(&paths, &workspace, &direct, mode))
                .await
                .unwrap();
            assert_eq!(actual, expected);
            for digest in actual.values() {
                assert_eq!(
                    direct.get_blob(digest).await.unwrap(),
                    staged.get_blob(digest).await.unwrap()
                );
            }
        }
    }
}
