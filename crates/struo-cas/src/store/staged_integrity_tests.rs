use super::*;

#[tokio::test]
async fn raw_and_compressed_staging_verify_before_exposing_a_reader() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path().join("cas"));
    for original in [
        b"trusted raw data".to_vec(),
        vec![b'x'; super::super::STREAM_CHUNK * 4],
    ] {
        let digest = cas.put_blob(&original).await.unwrap();
        let changed = vec![b'y'; original.len()];
        let encoded = blob::encode_bytes(&changed).unwrap();
        tokio::fs::write(cas.blob_path(&digest), encoded.as_ref())
            .await
            .unwrap();
        let error = cas.stage_blob(&digest, temp.path()).await.unwrap_err();
        assert!(error.is_corrupt(), "{error}");
        assert!(!cas.has_blob(&digest).await.unwrap());
        assert!(std::fs::read_dir(temp.path())
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "cas"));
    }
}
