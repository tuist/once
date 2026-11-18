use super::*;
use std::io::{self, Cursor};
use std::sync::Arc;

use crate::{ResourceLimits, ResourcePool, ResourceRequest};
use futures::FutureExt;

fn blob() -> Vec<u8> {
    let mut bytes = DIRECTORY_BLOB_MAGIC.to_vec();
    bytes.push(1);
    bytes.extend_from_slice(&5_u32.to_le_bytes());
    bytes.extend_from_slice(&0o751_u32.to_le_bytes());
    bytes.extend_from_slice(&7_u64.to_le_bytes());
    bytes.extend_from_slice(b"firstpayload");
    bytes
}

fn clean(parent: &Path) {
    assert!(!std::fs::read_dir(parent).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".struo-tree-")
    }));
}

#[tokio::test]
async fn prepared_tree_is_not_visible_until_caller_publication() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let Preparation::Ready(tree) = prepare("output", &destination, Cursor::new(blob()), ())
        .await
        .unwrap()
    else {
        panic!("fresh tree should prepare");
    };
    assert!(!destination.exists());
    tree.publish("output", &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("first")).unwrap(),
        b"payload"
    );
    clean(temp.path());
}

#[tokio::test]
async fn malformed_and_failed_publication_never_expose_private_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let mut bytes = blob();
    bytes.pop();
    assert!(prepare("output", &destination, Cursor::new(bytes), ())
        .await
        .is_err());
    assert!(!destination.exists());
    clean(temp.path());
    let Preparation::Ready(tree) = prepare("output", &destination, Cursor::new(blob()), ())
        .await
        .unwrap()
    else {
        panic!("fresh tree should prepare");
    };
    std::fs::create_dir(&destination).unwrap();
    assert!(tree.publish("output", &destination).is_err());
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
    clean(temp.path());
}

#[tokio::test]
async fn legacy_preparation_and_worker_panics_preserve_publication_boundaries() {
    struct Panicking;
    impl Read for Panicking {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("injected reader panic");
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let mut legacy = blob();
    legacy.remove(DIRECTORY_BLOB_MAGIC.len());
    legacy[..DIRECTORY_BLOB_V1_MAGIC.len()].copy_from_slice(DIRECTORY_BLOB_V1_MAGIC);
    let Preparation::Ready(tree) = prepare("output", &destination, Cursor::new(legacy), ())
        .await
        .unwrap()
    else {
        panic!("legacy fresh tree should prepare");
    };
    tree.publish("output", &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("first")).unwrap(),
        b"payload"
    );
    std::fs::remove_dir_all(&destination).unwrap();
    let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
        1,
        1024 * 1024 * 1024,
    )));
    let owner = pool.acquire(ResourceRequest::default()).await;
    assert!(prepare("output", &destination, Panicking, owner)
        .await
        .is_err());
    assert!(pool
        .acquire(ResourceRequest::default())
        .now_or_never()
        .is_some());
    assert!(!destination.exists());
    clean(temp.path());
}

#[tokio::test]
async fn existing_outputs_keep_the_original_reader_for_synchronous_restoration() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    std::fs::create_dir(&destination).unwrap();
    let Preparation::Unavailable(mut reader) =
        prepare("output", &destination, Cursor::new(blob()), ())
            .await
            .unwrap()
    else {
        panic!("existing tree must not prepare asynchronously");
    };
    assert_eq!(reader.position(), 0);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, blob());
    clean(temp.path());
}

#[tokio::test]
async fn cancellation_keeps_private_tree_and_reservation_until_worker_cleanup() {
    struct Gated {
        bytes: Cursor<Vec<u8>>,
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        wait: std::sync::mpsc::Receiver<()>,
    }
    impl Read for Gated {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.bytes.position() == u64::try_from(self.bytes.get_ref().len()).unwrap() {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                    self.wait
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                }
            }
            self.bytes.read(bytes)
        }
    }
    let workspace = Arc::new(tempfile::tempdir().unwrap());
    let destination = workspace.path().join("output");
    let path = destination.clone();
    let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
        1,
        1024 * 1024 * 1024,
    )));
    let permit = pool.acquire(ResourceRequest::default()).await;
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let parent = Arc::clone(&workspace);
    let task = tokio::spawn(crate::with_resource_permit(permit, async move {
        prepare(
            "output",
            &path,
            Gated {
                bytes: Cursor::new(blob()),
                entered: Some(entered),
                wait,
            },
            (parent, crate::resource_scope::current().unwrap()),
        )
        .await
    }));
    tokio::time::timeout(std::time::Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.is_err());
    assert!(pool
        .acquire(ResourceRequest::default())
        .now_or_never()
        .is_none());
    assert!(!destination.exists());
    let staged = std::fs::read_dir(workspace.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read(staged.join("tree/first")).unwrap(),
        b"payload"
    );
    release.send(()).unwrap();
    let next = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pool.acquire(ResourceRequest::default()),
    )
    .await
    .unwrap();
    drop(next);
    assert!(!destination.exists());
    clean(workspace.path());
}
