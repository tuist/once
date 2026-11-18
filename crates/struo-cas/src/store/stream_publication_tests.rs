use super::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::ReadBuf;

struct ErrorAfterBody<'a>(&'a [u8]);

impl AsyncRead for ErrorAfterBody<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.0.is_empty() {
            return Poll::Ready(Err(io::Error::other("body without EOF")));
        }
        let count = buffer.remaining().min(self.0.len());
        buffer.put_slice(&self.0[..count]);
        self.0 = &self.0[count..];
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn existing_content_still_requires_successful_stream_eof() {
    let root = tempfile::tempdir().unwrap();
    let cas = Cas::open(root.path());
    let digest = cas.put_blob(b"existing").await.unwrap();
    assert!(cas.put_stream(ErrorAfterBody(b"existing")).await.is_err());
    assert_eq!(cas.get_blob(&digest).await.unwrap(), b"existing");
    assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
}

#[tokio::test]
async fn existing_raw_and_encoded_blobs_are_not_replaced_by_stream_scratch() {
    for bytes in [
        b"raw".to_vec(),
        vec![b'x'; STREAM_CHUNK * 3],
        blob::ZSTD_BLOB_MAGIC.to_vec(),
    ] {
        let root = tempfile::tempdir().unwrap();
        let cas = Cas::open(root.path());
        let digest = cas.put_blob(&bytes).await.unwrap();
        let path = cas.blob_path(&digest);
        let before = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(cas.put_stream(bytes.as_slice()).await.unwrap(), digest);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(cas.get_blob(&digest).await.unwrap(), bytes);
        assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn prepared_stream_keeps_its_original_write_handle_for_durable_publication() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (path, digest, file) = stream::prepare(root.path(), b"prepared".as_slice())
        .await
        .unwrap();
    file.set_permissions(std::fs::Permissions::from_mode(0o400))
        .unwrap();
    File::from_std(file).sync_all().await.unwrap();
    assert_eq!(digest, Digest::of_bytes(b"prepared"));
    assert_eq!(std::fs::read(&path).unwrap(), b"prepared");
    drop(path);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
