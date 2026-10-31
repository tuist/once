use super::*;

#[tokio::test]
#[ignore = "requires the Docker CLI and openssl, no daemon required"]
async fn real_cli_snapshots_default_and_named_tls_contexts_without_downgrading_verification() {
    let binary = std::env::var_os("ONCE_DOCKER_BIN").unwrap_or_else(|| "docker".into());
    for named in [false, true] {
        for skip_verify in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let key = root.path().join("key.pem");
            let cert = root.path().join("cert.pem");
            let ca = root.path().join("ca.pem");
            let generated = Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-subj",
                    "/CN=localhost",
                    "-days",
                    "1",
                    "-keyout",
                ])
                .arg(&key)
                .arg("-out")
                .arg(&cert)
                .output()
                .await
                .unwrap();
            assert!(generated.status.success());
            std::fs::copy(&cert, &ca).unwrap();
            let mut client = Client::new(binary.clone(), RemoteIoLimits::default());
            let environment = &mut Arc::get_mut(&mut client.connection).unwrap().environment;
            environment.clear();
            environment.insert("DOCKER_CONFIG".into(), root.path().join("config").into());
            if named {
                client.control(&[
                    "context".into(), "create".into(), "secured".into(), "--docker".into(),
                    format!("host=tcp://127.0.0.1:1234,ca={},cert={},key={},skip-tls-verify={skip_verify}",
                        ca.display(), cert.display(), key.display()),
                ]).await.unwrap();
                Arc::get_mut(&mut client.connection)
                    .unwrap()
                    .environment
                    .insert("DOCKER_CONTEXT".into(), "secured".into());
            } else {
                environment.insert("DOCKER_HOST".into(), "tcp://127.0.0.1:1234".into());
                environment.insert("DOCKER_CERT_PATH".into(), root.path().as_os_str().into());
                environment.insert(
                    if skip_verify {
                        "DOCKER_TLS"
                    } else {
                        "DOCKER_TLS_VERIFY"
                    }
                    .into(),
                    "1".into(),
                );
            }
            let repeated = client.clone().snapshot().await.unwrap();
            let pinned = client.snapshot().await.unwrap();
            assert_eq!(pinned.credential(), repeated.credential());
            drop(repeated);
            let config = pinned.connection.config.as_ref().unwrap().path().to_owned();
            let clone = pinned.clone();
            drop(pinned);
            assert!(
                config.is_dir(),
                "a recovery clone must retain its TLS snapshot"
            );
            let inspected = clone
                .control(&["context".into(), "inspect".into(), "once-pinned".into()])
                .await
                .unwrap();
            let inspected: serde_json::Value = serde_json::from_slice(&inspected).unwrap();
            assert_eq!(
                inspected[0]["Endpoints"]["docker"]["Host"],
                "tcp://127.0.0.1:1234"
            );
            assert_eq!(
                inspected[0]["Endpoints"]["docker"]["SkipTLSVerify"],
                skip_verify
            );
            let tls = std::path::Path::new(inspected[0]["Storage"]["TLSPath"].as_str().unwrap())
                .join("docker");
            for original in [&ca, &cert, &key] {
                assert_eq!(
                    std::fs::read(tls.join(original.file_name().unwrap())).unwrap(),
                    std::fs::read(original).unwrap()
                );
                std::fs::remove_file(original).unwrap();
            }
            clone
                .control(&["context".into(), "inspect".into(), "once-pinned".into()])
                .await
                .unwrap();
            drop(clone);
            assert!(
                !config.exists(),
                "snapshot credentials must be removed after the last owner"
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn failed_context_export_cannot_fall_back_to_the_mutable_configuration() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("docker");
    std::fs::write(
        &binary,
        "#!/bin/sh\ncase \"$2\" in show) printf 'selected\\n';; *) exit 1;; esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(Client::new(binary.into(), RemoteIoLimits::default())
        .connect()
        .await
        .is_err());
}
