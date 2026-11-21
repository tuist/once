//! S3 credentials from the AWS SDK's default chain, so named profiles, IAM
//! Identity Center (SSO) sessions, `credential_process`, and assumed roles in
//! `~/.aws` work the way they do for the AWS CLI.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::profile::ProfileFileRegionProvider;
use aws_credential_types::provider::error::CredentialsError;
use aws_credential_types::provider::ProvideCredentials;
use object_store::aws::AwsCredential;
use object_store::CredentialProvider;
use tokio::sync::{Mutex, OnceCell};
use tracing::instrument::WithSubscriber;

/// Credentials are renewed this long before they expire, so a request
/// signed with them does not outlive them.
const REFRESH_MARGIN: Duration = Duration::from_mins(5);

/// Resolves credentials through the AWS default chain on first use and
/// caches them until shortly before they expire.
pub(super) struct AwsChainCredentials {
    profile: Option<String>,
    chain: OnceCell<Box<dyn ProvideCredentials>>,
    cached: Mutex<Option<(Arc<AwsCredential>, Option<SystemTime>)>>,
}

impl std::fmt::Debug for AwsChainCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsChainCredentials")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

impl AwsChainCredentials {
    /// Use `profile`, or `AWS_PROFILE` and then `default` when it is `None`.
    pub(super) fn new(profile: Option<String>) -> Self {
        Self {
            profile,
            chain: OnceCell::new(),
            cached: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(super) fn with_provider(provider: impl ProvideCredentials + 'static) -> Self {
        Self {
            profile: None,
            chain: OnceCell::new_with(Some(Box::new(provider))),
            cached: Mutex::new(None),
        }
    }

    async fn chain(&self) -> &dyn ProvideCredentials {
        self.chain
            .get_or_init(|| async {
                let mut builder = DefaultCredentialsChain::builder();
                if let Some(profile) = &self.profile {
                    builder = builder.profile_name(profile);
                }
                Box::new(builder.build().await) as Box<dyn ProvideCredentials>
            })
            .await
            .as_ref()
    }
}

#[async_trait::async_trait]
impl CredentialProvider for AwsChainCredentials {
    type Credential = AwsCredential;

    async fn get_credential(&self) -> object_store::Result<Arc<AwsCredential>> {
        let mut cached = self.cached.lock().await;
        if let Some((credential, expiry)) = cached.as_ref() {
            let fresh = expiry.is_none_or(|expiry| {
                expiry
                    .duration_since(SystemTime::now())
                    .is_ok_and(|left| left > REFRESH_MARGIN)
            });
            if fresh {
                return Ok(Arc::clone(credential));
            }
        }
        // SDK credential diagnostics can include credential_process stderr.
        let loaded = async { self.chain().await.provide_credentials().await }
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map_err(|error| credentials_error(&error))?;
        let credential = Arc::new(AwsCredential {
            key_id: loaded.access_key_id().to_string(),
            secret_key: loaded.secret_access_key().to_string(),
            token: loaded.session_token().map(str::to_string),
        });
        *cached = Some((Arc::clone(&credential), loaded.expiry()));
        Ok(credential)
    }
}

/// Missing or invalid configuration is a refusal: the command should fail
/// rather than quietly run without the shared cache. A provider that could
/// not be reached, such as the SSO portal or STS while offline, is an outage.
/// An expired SSO session also reports as a provider error, so it falls back
/// to local work with a warning that says to sign in again.
fn credentials_error(error: &CredentialsError) -> object_store::Error {
    match error {
        CredentialsError::CredentialsNotLoaded(_) | CredentialsError::InvalidConfiguration(_) => {
            object_store::Error::Unauthenticated {
                path: String::new(),
                source: "AWS credentials are missing or misconfigured; check the selected profile".into(),
            }
        }
        _ if sts_refused(error) => object_store::Error::Unauthenticated {
            path: String::new(),
            source: "AWS STS refused credentials; check the source identity and role permissions".into(),
        },
        _ => object_store::Error::Generic {
            store: "S3",
            source: "AWS credential provider failed; check connectivity and renew the session with aws sso login if applicable".into(),
        },
    }
}

fn sts_refused(error: &(dyn std::error::Error + 'static)) -> bool {
    use aws_sdk_sts::error::ProvideErrorMetadata;
    use aws_sdk_sts::operation::assume_role::AssumeRoleError;
    use aws_sdk_sts::operation::assume_role_with_web_identity::AssumeRoleWithWebIdentityError;

    let mut source = Some(error);
    while let Some(error) = source {
        let code = error
            .downcast_ref::<AssumeRoleError>()
            .and_then(ProvideErrorMetadata::code)
            .or_else(|| {
                error
                    .downcast_ref::<AssumeRoleWithWebIdentityError>()
                    .and_then(ProvideErrorMetadata::code)
            });
        if matches!(
            code,
            Some(
                "AccessDenied"
                    | "AccessDeniedException"
                    | "InvalidClientTokenId"
                    | "UnrecognizedClientException"
                    | "SignatureDoesNotMatch"
                    | "ExpiredToken"
                    | "ExpiredTokenException"
                    | "InvalidIdentityToken"
                    | "IDPRejectedClaim"
            )
        ) {
            return true;
        }
        source = error.source();
    }
    false
}

/// The region `profile` (or `AWS_PROFILE`) names in `~/.aws/config`, read
/// without network access. Runs on its own thread because callers build
/// stores synchronously, possibly inside an async runtime.
pub(super) fn profile_region(profile: Option<&str>) -> Option<String> {
    let mut builder = ProfileFileRegionProvider::builder();
    if let Some(profile) = profile {
        builder = builder.profile_name(profile);
    }
    region_from(builder.build())
}

pub(super) fn region_from(provider: ProfileFileRegionProvider) -> Option<String> {
    use aws_config::meta::region::ProvideRegion;
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                runtime
                    .block_on(provider.region())
                    .map(|region| region.to_string())
            })
            .join()
            .ok()
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aws_credential_types::provider::future;
    use aws_credential_types::Credentials;
    use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};

    use super::*;

    #[derive(Debug)]
    struct Counting {
        calls: Arc<AtomicUsize>,
        lifetime: Option<Duration>,
    }

    impl ProvideCredentials for Counting {
        fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
        where
            Self: 'a,
        {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            future::ProvideCredentials::ready(Ok(Credentials::new(
                format!("key-{call}"),
                "secret",
                Some("session".to_string()),
                self.lifetime.map(|lifetime| SystemTime::now() + lifetime),
                "test",
            )))
        }
    }

    async fn key_ids(lifetime: Option<Duration>) -> (Vec<String>, usize) {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = AwsChainCredentials::with_provider(Counting {
            calls: Arc::clone(&calls),
            lifetime,
        });
        let mut keys = Vec::new();
        for _ in 0..3 {
            keys.push(provider.get_credential().await.unwrap().key_id.clone());
        }
        (keys, calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn credentials_are_reused_until_they_near_expiry() {
        assert_eq!(key_ids(None).await.1, 1);
        assert_eq!(key_ids(Some(Duration::from_secs(3600))).await.1, 1);
        let (keys, calls) = key_ids(Some(Duration::from_secs(60))).await;
        assert_eq!(calls, 3);
        assert_eq!(keys, ["key-0", "key-1", "key-2"]);
    }

    #[tokio::test]
    async fn session_tokens_are_kept() {
        let provider = AwsChainCredentials::with_provider(Credentials::new(
            "id",
            "secret",
            Some("token".into()),
            None,
            "test",
        ));
        let credential = provider.get_credential().await.unwrap();
        assert_eq!(credential.token.as_deref(), Some("token"));
        assert_eq!(credential.secret_key, "secret");
    }

    #[test]
    fn missing_credentials_are_refused_and_provider_failures_are_outages() {
        assert!(matches!(
            credentials_error(&CredentialsError::not_loaded("no profile")),
            object_store::Error::Unauthenticated { .. }
        ));
        assert!(matches!(
            credentials_error(&CredentialsError::invalid_configuration("bad profile")),
            object_store::Error::Unauthenticated { .. }
        ));
        let outage = credentials_error(&CredentialsError::provider_error("portal unreachable"));
        assert!(matches!(outage, object_store::Error::Generic { .. }));
        assert!(outage.to_string().contains("aws sso login"), "{outage}");
        assert!(!outage.to_string().contains("portal unreachable"));
    }

    #[test]
    fn nested_sts_refusals_fail_without_exposing_service_messages() {
        use aws_sdk_sts::error::ErrorMetadata;
        use aws_sdk_sts::operation::assume_role::AssumeRoleError;
        for (code, refused) in [
            ("AccessDenied", true),
            ("InvalidClientTokenId", true),
            ("Throttling", false),
            ("ServiceUnavailable", false),
        ] {
            let service = AssumeRoleError::generic(
                ErrorMetadata::builder()
                    .code(code)
                    .message("SECRET_SENTINEL")
                    .build(),
            );
            let wrapped = CredentialsError::provider_error(
                aws_sdk_sts::error::SdkError::service_error(service, ()),
            );
            let error = credentials_error(&wrapped);
            assert_eq!(
                matches!(error, object_store::Error::Unauthenticated { .. }),
                refused,
                "{code}"
            );
            assert!(!error.to_string().contains("SECRET_SENTINEL"));
        }
        let service = aws_sdk_sts::operation::assume_role_with_web_identity::AssumeRoleWithWebIdentityError::generic(
            ErrorMetadata::builder().code("InvalidIdentityToken").build(),
        );
        let wrapped = CredentialsError::provider_error(
            aws_sdk_sts::error::SdkError::service_error(service, ()),
        );
        assert!(matches!(
            credentials_error(&wrapped),
            object_store::Error::Unauthenticated { .. }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failing_credential_process_does_not_leak_stderr_to_errors_or_logs() {
        use aws_config::profile::ProfileFileCredentialsProvider;
        let files = EnvConfigFiles::builder()
            .with_contents(EnvConfigFileKind::Config, "[profile secret-test]\ncredential_process = /bin/sh -c 'echo SECRET_SENTINEL >&2; exit 1'\n")
            .build();
        let provider = AwsChainCredentials::with_provider(
            ProfileFileCredentialsProvider::builder()
                .profile_files(files)
                .profile_name("secret-test")
                .build(),
        );
        let (captured, _guard) = crate::tuist::log_capture::capture();
        let error = provider.get_credential().await.unwrap_err();
        tracing::warn!(%error, "credential lookup failed");
        assert!(!error.to_string().contains("SECRET_SENTINEL"));
        assert!(
            !captured.text().contains("SECRET_SENTINEL"),
            "credential process diagnostics leaked"
        );
        assert!(captured.text().contains("credential lookup failed"));
    }

    #[test]
    fn regions_come_from_the_named_profile() {
        let files = || {
            EnvConfigFiles::builder()
                .with_contents(
                    EnvConfigFileKind::Config,
                    "[default]\nregion = us-east-2\n[profile dev]\nregion = eu-west-1\n",
                )
                .build()
        };
        let region = |name: &str| {
            region_from(
                ProfileFileRegionProvider::builder()
                    .profile_files(files())
                    .profile_name(name)
                    .build(),
            )
        };
        assert_eq!(region("dev").as_deref(), Some("eu-west-1"));
        assert_eq!(region("default").as_deref(), Some("us-east-2"));
        assert_eq!(region("missing"), None);
    }
}
