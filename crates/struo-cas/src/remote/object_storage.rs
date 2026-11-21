//! [`RemoteStore`] over S3-compatible, Google Cloud Storage, and Azure Blob
//! Storage buckets, plus local directories, through the `object_store` crate.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use futures::TryStreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, ObjectStoreScheme, PutPayload, WriteMultipart};
use reqwest::Url;
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;

use super::aws_credentials::{self, AwsChainCredentials};
use super::{RemoteBody, RemoteStore};
use crate::{Error, ErrorKind, Result};

/// Namespace for the key layout and record format this client writes.
const LAYOUT_VERSION: &str = "v1";
const MIN_PART_BYTES: u64 = 8 * 1024 * 1024;
/// S3 accepts at most 10,000 parts; leave headroom.
const MAX_PARTS: u64 = 9_000;
const PARTS_IN_FLIGHT: usize = 2;
const READ_CHUNK: usize = 256 * 1024;

/// A bucket, container, or directory addressed by URL.
#[derive(Debug)]
pub struct ObjectStoreRemote {
    pub(super) store: Arc<dyn ObjectStore>,
    prefix: ObjectPath,
    provider: &'static str,
}

impl ObjectStoreRemote {
    /// Open the store `url` names, such as `s3://bucket/prefix`,
    /// `gs://bucket/prefix`, `az://container/prefix`, or
    /// `file:///absolute/directory`.
    ///
    /// Cloud credentials and settings come from the environment variables
    /// each service's SDKs read, such as `AWS_ACCESS_KEY_ID`; `options`
    /// override them with `object_store` configuration keys, such as
    /// `endpoint` or `region`. S3 credentials resolve through the AWS SDK's
    /// default chain, so `options.profile` or `AWS_PROFILE` can name a
    /// profile, including an SSO session, in `~/.aws`. No request is sent
    /// until first use.
    pub fn from_url(url: &str, options: &BTreeMap<String, String>) -> Result<Self> {
        if let Some(key) = options.keys().find(|key| is_credential(key)) {
            return Err(misconfigured(format!(
                "option `{key}` is a credential; set it in the environment instead, such as \
                 AWS_SECRET_ACCESS_KEY or AZURE_STORAGE_ACCOUNT_KEY, so it never lands in a \
                 configuration file"
            )));
        }
        let parsed =
            Url::parse(url).map_err(|_| misconfigured("invalid object store URL".to_string()))?;
        let azure_container = matches!(parsed.scheme(), "az" | "abfs" | "abfss");
        if parsed.password().is_some() || (!parsed.username().is_empty() && !azure_container) {
            return Err(misconfigured(
                "URL credentials are not supported; configure credentials outside the URL"
                    .to_string(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(misconfigured(
                "object store URL must not contain a query or fragment".to_string(),
            ));
        }
        let (scheme, path) = ObjectStoreScheme::parse(&parsed)
            .map_err(|_| misconfigured("invalid or unsupported object store URL".to_string()))?;
        let (provider, store): (&'static str, Arc<dyn ObjectStore>) = match scheme {
            ObjectStoreScheme::AmazonS3 => ("s3", Arc::new(s3_store(url, options)?)),
            ObjectStoreScheme::GoogleCloudStorage => (
                "gcs",
                Arc::new(
                    configure(
                        GoogleCloudStorageBuilder::from_env()
                            .with_url(url)
                            .with_retry(retry_config()),
                        options,
                        |b, k, v| Ok(b.with_config(k.parse()?, v)),
                    )?
                    .build()
                    .map_err(|error| build_error(&error))?,
                ),
            ),
            ObjectStoreScheme::MicrosoftAzure => (
                "azure",
                Arc::new(
                    configure(
                        MicrosoftAzureBuilder::from_env()
                            .with_url(url)
                            .with_retry(retry_config()),
                        options,
                        |b, k, v| Ok(b.with_config(k.parse()?, v)),
                    )?
                    .build()
                    .map_err(|error| build_error(&error))?,
                ),
            ),
            ObjectStoreScheme::Local => {
                if let Some(key) = options.keys().next() {
                    return Err(misconfigured(format!(
                        "`file://` stores take no options, found `{key}`"
                    )));
                }
                ("file", Arc::new(LocalFileSystem::new()))
            }
            _ => {
                return Err(misconfigured(
                    "unsupported object store URL; use s3://, gs://, az://, or \
                     file://, and set `endpoint` in options for S3-compatible services"
                        .to_string(),
                ))
            }
        };
        Ok(Self::new(store, path, provider))
    }

    pub(crate) fn new(
        store: Arc<dyn ObjectStore>,
        prefix: ObjectPath,
        provider: &'static str,
    ) -> Self {
        Self {
            store,
            prefix,
            provider,
        }
    }

    pub(super) fn location(&self, key: &str) -> ObjectPath {
        key.split('/')
            .fold(self.prefix.clone().join(LAYOUT_VERSION), |path, part| {
                path.join(part)
            })
    }

    fn error(&self, operation: &'static str, error: &object_store::Error) -> Error {
        let kind = match &error {
            object_store::Error::Unauthenticated { .. }
            | object_store::Error::PermissionDenied { .. } => ErrorKind::Refused,
            object_store::Error::InvalidPath { .. }
            | object_store::Error::NotSupported { .. }
            | object_store::Error::NotImplemented { .. }
            | object_store::Error::UnknownConfigurationKey { .. } => ErrorKind::Misconfigured,
            object_store::Error::NotFound { .. } => ErrorKind::NotFound,
            // Requests that reach here exhausted the store's own retries.
            _ => ErrorKind::Unavailable,
        };
        Error::remote(self.provider, operation, kind, error.to_string())
    }

    fn part_bytes(size: u64) -> u64 {
        MIN_PART_BYTES.max(size.div_ceil(MAX_PARTS))
    }
}

#[async_trait::async_trait]
impl RemoteStore for ObjectStoreRemote {
    fn provider(&self) -> &'static str {
        self.provider
    }

    async fn head(&self, key: &str) -> Result<Option<u64>> {
        match self.store.head(&self.location(key)).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(self.error("head", &error)),
        }
    }

    async fn get(&self, key: &str) -> Result<Option<RemoteBody>> {
        let result = match self.store.get(&self.location(key)).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(self.error("get", &error)),
        };
        let size = result.meta.size;
        let stream = result.into_stream().map_err(std::io::Error::other);
        Ok(Some(RemoteBody {
            size,
            reader: Box::pin(StreamReader::new(stream)),
        }))
    }

    async fn put_bytes(&self, key: &str, bytes: Bytes) -> Result<()> {
        self.store
            .put(&self.location(key), PutPayload::from(bytes))
            .await
            .map(|_| ())
            .map_err(|error| self.error("put", &error))
    }

    async fn put_file(&self, key: &str, path: &Path, size: u64) -> Result<()> {
        let io_error = |source| Error::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut file = tokio::fs::File::open(path).await.map_err(io_error)?;
        let part = Self::part_bytes(size);
        if size <= part {
            let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or_default());
            (&mut file)
                .take(size.saturating_add(1))
                .read_to_end(&mut bytes)
                .await
                .map_err(io_error)?;
            if bytes.len() as u64 != size {
                return Err(io_error(std::io::Error::other(
                    "file changed size during upload",
                )));
            }
            return self.put_bytes(key, Bytes::from(bytes)).await;
        }
        let location = self.location(key);
        let upload = self
            .store
            .put_multipart(&location)
            .await
            .map_err(|error| self.error("put", &error))?;
        let mut writer = WriteMultipart::new_with_chunk_size(
            upload,
            usize::try_from(part).unwrap_or(usize::MAX),
        );
        let mut buffer = vec![0; READ_CHUNK];
        let mut sent = 0_u64;
        let outcome = async {
            loop {
                let count = file.read(&mut buffer).await.map_err(io_error)?;
                if count == 0 {
                    break;
                }
                sent = sent.saturating_add(count as u64);
                if sent > size {
                    return Err(io_error(std::io::Error::other(
                        "file changed size during upload",
                    )));
                }
                writer
                    .wait_for_capacity(PARTS_IN_FLIGHT)
                    .await
                    .map_err(|error| self.error("put", &error))?;
                writer.write(&buffer[..count]);
            }
            if sent != size {
                return Err(io_error(std::io::Error::other(
                    "file changed size during upload",
                )));
            }
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => writer
                .finish()
                .await
                .map(|_| ())
                .map_err(|error| self.error("put", &error)),
            Err(error) => {
                if let Err(abort) = writer.abort().await {
                    tracing::warn!(provider = self.provider, error = %abort, "could not abort multipart upload");
                }
                Err(error)
            }
        }
    }

    fn upload_memory(&self, size: u64) -> u64 {
        let part = Self::part_bytes(size);
        if size <= part {
            size
        } else {
            // Parts in flight, the part being filled, and the read buffer.
            part.saturating_mul(PARTS_IN_FLIGHT as u64 + 1)
                .saturating_add(READ_CHUNK as u64)
        }
    }
}

fn configure<B>(
    builder: B,
    options: &BTreeMap<String, String>,
    apply: impl Fn(B, &str, &str) -> std::result::Result<B, object_store::Error>,
) -> Result<B> {
    options.iter().try_fold(builder, |builder, (key, value)| {
        let key = key.to_ascii_lowercase();
        apply(builder, &key, value)
            .map_err(|_| misconfigured(format!("unknown object store option `{key}`")))
    })
}

/// Options `object_store` reads to find S3 credentials itself. When any is
/// set, it keeps that responsibility instead of the AWS SDK chain.
const S3_CREDENTIAL_SOURCES: [&str; 6] = [
    "skip_signature",
    "web_identity_token_file",
    "role_arn",
    "container_credentials_relative_uri",
    "container_credentials_full_uri",
    "metadata_endpoint",
];

fn s3_store(url: &str, options: &BTreeMap<String, String>) -> Result<object_store::aws::AmazonS3> {
    let mut options: BTreeMap<String, String> = options
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
        .collect();
    let profile = options
        .remove("profile")
        .or_else(|| options.remove("aws_profile"))
        .filter(|profile| !profile.trim().is_empty());
    let mut builder = AmazonS3Builder::from_env()
        .with_url(url)
        .with_retry(retry_config());
    let own_source = options.keys().any(|key| {
        let key = key.strip_prefix("aws_").unwrap_or(key);
        S3_CREDENTIAL_SOURCES.contains(&key)
    });
    if own_source {
        if profile.is_some() {
            return Err(misconfigured(
                "`profile` cannot be combined with options that select another credential \
                 source, such as `skip_signature` or `role_arn`"
                    .to_string(),
            ));
        }
    } else {
        let region_configured = [
            "region",
            "aws_region",
            "default_region",
            "aws_default_region",
        ]
        .iter()
        .any(|key| options.contains_key(*key))
            || ["AWS_REGION", "AWS_DEFAULT_REGION"]
                .iter()
                .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
        if !region_configured {
            if let Some(region) = aws_credentials::profile_region(profile.as_deref()) {
                builder = builder.with_region(region);
            }
        }
        builder = builder.with_credentials(Arc::new(AwsChainCredentials::new(profile)));
    }
    configure(
        builder,
        &options,
        |b, k, v| Ok(b.with_config(k.parse()?, v)),
    )?
    .build()
    .map_err(|error| build_error(&error))
}

fn is_credential(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    // Paths and URIs locate credentials without containing them.
    if ["_file", "_path", "_uri"]
        .iter()
        .any(|suffix| key.ends_with(suffix))
    {
        return false;
    }
    [
        "secret",
        "access_key",
        "account_key",
        "master_key",
        "sas",
        "token",
        "service_account_key",
        "password",
    ]
    .iter()
    .any(|marker| key.contains(marker))
}

/// A cache that cannot answer should fall back to local work quickly rather
/// than hold every action through `object_store`'s three-minute default.
fn retry_config() -> object_store::RetryConfig {
    object_store::RetryConfig {
        backoff: object_store::BackoffConfig::default(),
        max_retries: 3,
        retry_timeout: std::time::Duration::from_secs(30),
    }
}

fn build_error(error: &object_store::Error) -> Error {
    misconfigured(error.to_string())
}

fn misconfigured(message: String) -> Error {
    Error::InvalidConfig {
        provider: "object_store",
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_live_under_the_prefix_and_layout_version() {
        let remote = ObjectStoreRemote::new(
            Arc::new(object_store::memory::InMemory::new()),
            ObjectPath::from("team/cache"),
            "memory",
        );
        assert_eq!(
            remote.location("cas/abcd").as_ref(),
            "team/cache/v1/cas/abcd"
        );
    }

    #[test]
    fn urls_select_their_service() {
        for (url, option, provider) in [
            ("s3://bucket/prefix", ("region", "eu-west-1"), "s3"),
            ("s3://bucket/prefix", ("profile", "dev"), "s3"),
            ("gs://bucket/prefix", ("bucket", "bucket"), "gcs"),
            (
                "az://container/prefix",
                ("account_name", "devstore"),
                "azure",
            ),
        ] {
            let options = BTreeMap::from([(option.0.to_string(), option.1.to_string())]);
            let remote = ObjectStoreRemote::from_url(url, &options)
                .unwrap_or_else(|error| panic!("{url}: {error}"));
            assert_eq!(remote.provider(), provider);
            assert_eq!(remote.prefix.as_ref(), "prefix");
        }
    }

    #[test]
    fn unknown_options_and_schemes_are_misconfigured() {
        let bad = BTreeMap::from([("regoin".to_string(), "eu-west-1".to_string())]);
        let error = ObjectStoreRemote::from_url("s3://bucket", &bad).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Misconfigured);
        assert!(error.to_string().contains("regoin"), "{error}");

        for key in [
            "secret_access_key",
            "AWS_SECRET_ACCESS_KEY",
            "account_key",
            "sas_token",
        ] {
            let options = BTreeMap::from([(key.to_string(), "x".to_string())]);
            let error = ObjectStoreRemote::from_url("s3://bucket", &options).unwrap_err();
            assert!(error.to_string().contains("environment"), "{key}: {error}");
        }

        for key in ["web_identity_token_file", "aws_web_identity_token_file"] {
            assert!(!is_credential(key), "{key}");
        }
        let options = BTreeMap::from([
            ("profile".to_string(), "dev".to_string()),
            ("skip_signature".to_string(), "true".to_string()),
        ]);
        let error = ObjectStoreRemote::from_url("s3://bucket", &options).unwrap_err();
        assert!(error.to_string().contains("profile"), "{error}");

        let error = ObjectStoreRemote::from_url("ftp://host/x", &BTreeMap::new()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Misconfigured);
    }

    #[test]
    fn large_uploads_stay_within_the_part_limit() {
        let size: u64 = 200 * 1024 * 1024 * 1024;
        assert!(size.div_ceil(ObjectStoreRemote::part_bytes(size)) <= MAX_PARTS);
        assert_eq!(ObjectStoreRemote::part_bytes(1), MIN_PART_BYTES);
    }

    #[test]
    fn invalid_urls_do_not_echo_secrets() {
        for url in [
            "s3://user:private-password@invalid host/",
            "s3://user:private-password@bucket/",
            "gs://private-password@bucket/",
            "az://container:private-password@account.blob.core.windows.net/",
            "s3://bucket/?token=private-password",
            "gs://bucket/#private-password",
            "ftp://host/private-password",
            "gs://bucket/private-password/%FF",
        ] {
            let error = ObjectStoreRemote::from_url(url, &BTreeMap::new()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Misconfigured);
            assert!(!format!("{error:?} {error}").contains("private-password"));
        }
        let remote = ObjectStoreRemote::from_url(
            "abfss://container@account.dfs.core.windows.net/prefix",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(remote.provider(), "azure");
        assert_eq!(remote.prefix.as_ref(), "prefix");
    }
}
