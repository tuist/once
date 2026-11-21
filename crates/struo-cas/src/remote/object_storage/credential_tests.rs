use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpResponse, HttpService,
};
use object_store::ClientOptions;

use super::*;

#[derive(Debug, Clone)]
struct TokenEndpoint(Option<u16>);

impl HttpConnector for TokenEndpoint {
    fn connect(&self, _: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(client::CredentialService(HttpClient::new(
            self.clone(),
        ))))
    }
}

#[async_trait::async_trait]
impl HttpService for TokenEndpoint {
    async fn call(&self, request: HttpRequest) -> std::result::Result<HttpResponse, HttpError> {
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        match self.0 {
            Some(status) => Ok(axum::http::Response::builder()
                .status(status)
                .body(
                    r#"{"error":"invalid_client","error_description":"OAUTH_SECRET_SENTINEL"}"#
                        .to_string()
                        .into(),
                )
                .unwrap()),
            None => Err(HttpError::new(
                HttpErrorKind::Connect,
                std::io::Error::other("OAUTH_SECRET_SENTINEL"),
            )),
        }
    }
}

#[tokio::test]
async fn oauth_refusals_and_outages_keep_their_category_without_exposing_diagnostics() {
    let adc = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(adc.path(), r#"{"type":"authorized_user","client_id":"id","client_secret":"secret","refresh_token":"refresh"}"#).unwrap();
    for (status, kind) in [
        (Some(400), ErrorKind::Refused),
        (Some(401), ErrorKind::Refused),
        (Some(403), ErrorKind::Refused),
        (Some(429), ErrorKind::Unavailable),
        (Some(503), ErrorKind::Unavailable),
        (None, ErrorKind::Unavailable),
    ] {
        let retry = object_store::RetryConfig {
            max_retries: 0,
            ..retry_config()
        };
        let azure = MicrosoftAzureBuilder::new()
            .with_account("account")
            .with_container_name("cache")
            .with_client_id("id")
            .with_client_secret("secret")
            .with_tenant_id("tenant")
            .with_http_connector(TokenEndpoint(status))
            .with_retry(retry.clone())
            .build()
            .unwrap();
        let gcs = GoogleCloudStorageBuilder::new()
            .with_bucket_name("cache")
            .with_application_credentials(adc.path().to_str().unwrap())
            .with_http_connector(TokenEndpoint(status))
            .with_retry(retry)
            .build()
            .unwrap();
        for (store, provider) in [
            (Arc::new(azure) as Arc<dyn ObjectStore>, "azure"),
            (Arc::new(gcs), "gcs"),
        ] {
            let remote = ObjectStoreRemote::new(store, ObjectPath::from("p"), provider);
            let (logs, _guard) = crate::tuist::log_capture::capture();
            let error = remote.head("cas/key").await.unwrap_err();
            tracing::warn!(%error, "credential lookup failed");
            assert_eq!(error.kind(), kind, "{provider}: {status:?}");
            assert!(!format!("{error:?} {error}").contains("OAUTH_SECRET_SENTINEL"));
            assert!(!logs.text().contains("OAUTH_SECRET_SENTINEL"));
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn azure_cli_failure_keeps_helper_stderr_out_of_errors_and_logs() {
    const CHILD: &str = "STRUO_TEST_AZ_HELPER_DIR";
    let Some(directory) = std::env::var_os(CHILD) else {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("az");
        std::fs::write(&helper, "#!/bin/sh\nprintf called > \"$STRUO_TEST_AZ_HELPER_DIR/called\"\nprintf AZURE_SECRET_SENTINEL >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "remote::object_storage::credential_tests::azure_cli_failure_keeps_helper_stderr_out_of_errors_and_logs"])
            .env(CHILD, directory.path()).env("PATH", directory.path()).output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(directory.path().join("called")).unwrap(),
            b"called"
        );
        return;
    };
    let azure = MicrosoftAzureBuilder::new()
        .with_account("account")
        .with_container_name("cache")
        .with_use_azure_cli(true)
        .with_http_connector(client::CredentialConnector)
        .build()
        .unwrap();
    let remote = ObjectStoreRemote::new(Arc::new(azure), ObjectPath::from("p"), "azure");
    let (logs, _guard) = crate::tuist::log_capture::capture();
    let error = remote.head("cas/key").await.unwrap_err();
    tracing::warn!(%error, "credential lookup failed");
    assert_eq!(error.kind(), ErrorKind::Unavailable);
    assert!(!format!("{error:?} {error}").contains("AZURE_SECRET_SENTINEL"));
    assert!(!logs.text().contains("AZURE_SECRET_SENTINEL"));
    assert!(logs.text().contains("credential lookup failed"));
    assert!(Path::new(&directory).join("called").exists());
}
