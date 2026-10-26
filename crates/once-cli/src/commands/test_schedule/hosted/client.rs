use std::path::Path;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use once_cas::TuistAuth;
use once_core::Xdg;
use reqwest::{Client, Response, Url};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Host {
    client: Client,
    storage: Client,
    base: Url,
    auth: TuistAuth,
}

impl Host {
    pub fn resolve(workspace: &Path) -> Result<Self> {
        let xdg = Xdg::from_env();
        let crate::cache_provider::ResolvedCacheProviderConfig::Tuist(config) =
            crate::cache_provider::resolve_config(workspace, &xdg)?
        else {
            anyhow::bail!("hosted test sharding requires a configured Tuist project in [infrastructure.cache]");
        };
        let account = config
            .account
            .as_deref()
            .context("hosted sharding requires a Tuist account")?;
        let project = config
            .project
            .as_deref()
            .context("hosted sharding requires a Tuist project")?;
        let mut base = Url::parse(&format!("{}/", config.url.trim_end_matches('/')))?;
        base.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("invalid hosting URL"))?
            .pop_if_empty()
            .extend(["api", "projects", account, project, "tests", ""]);
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_mins(1))
            .user_agent(concat!("once/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let storage = Client::builder().timeout(Duration::from_mins(2)).build()?;
        Ok(Self {
            client,
            storage,
            base,
            auth: TuistAuth::new(crate::cache_provider::credentials_root(&xdg), &config),
        })
    }

    async fn token(&self) -> Result<String> {
        let auth = self.auth.clone();
        tokio::task::spawn_blocking(move || auth.token())
            .await?
            .map_err(Into::into)
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let response = self
            .client
            .post(self.base.join(path)?)
            .bearer_auth(self.token().await?)
            .json(body)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        read_json(response).await
    }

    pub async fn assignment(&self, id: &str, index: u32) -> Result<Value> {
        let response = self
            .client
            .get(self.base.join(&format!("shards/{id}/{index}"))?)
            .query(&[("shard_plan_id", id)])
            .bearer_auth(self.token().await?)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        read_json(response).await
    }

    pub async fn upload(&self, file: &Path, id: &str, reference: &str) -> Result<()> {
        let binding = json!({"shard_plan_id": id, "reference": reference});
        let start = self.post("shards/upload/start", &binding).await?;
        let upload_id = start["data"]["upload_id"]
            .as_str()
            .context("missing upload ID")?;
        let mut part = binding.clone();
        part["upload_id"] = upload_id.into();
        part["part_number"] = 1.into();
        let signed = self.post("shards/upload/generate-url", &part).await?;
        let url = storage_url(
            signed["data"]["url"]
                .as_str()
                .context("missing upload URL")?,
        )?;
        let file = tokio::fs::File::open(file).await?;
        let size = file.metadata().await?.len();
        let stream = futures::stream::try_unfold(file, |mut file| async move {
            let mut buffer = vec![0; 64 * 1024];
            let size = file.read(&mut buffer).await?;
            buffer.truncate(size);
            Ok::<_, std::io::Error>((size > 0).then_some((buffer, file)))
        });
        let response = self
            .storage
            .put(url)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        ensure!(
            response.status().is_success(),
            "hosted plan upload failed (HTTP {})",
            response.status().as_u16()
        );
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .context("plan upload omitted ETag")?
            .to_str()?;
        let mut complete = binding;
        complete["upload_id"] = upload_id.into();
        complete["parts"] = json!([{"part_number": 1, "etag": etag}]);
        self.post("shards/upload/complete", &complete).await?;
        Ok(())
    }

    pub async fn download(&self, url: &str, path: &Path) -> Result<()> {
        let response = self
            .storage
            .get(storage_url(url)?)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        stage(response, path, super::artifact::MAX_ARTIFACT_BYTES).await
    }
}

fn storage_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("invalid plan storage URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none(),
        "invalid plan storage URL"
    );
    Ok(url)
}

async fn stage(mut response: Response, path: &Path, limit: u64) -> Result<()> {
    ensure!(
        response.status().is_success(),
        "hosted sharding request failed (HTTP {})",
        response.status().as_u16()
    );
    ensure!(
        response.content_length().is_none_or(|size| size <= limit),
        "hosted response exceeds size limit"
    );
    let mut file = tokio::fs::File::create(path).await?;
    let mut size = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        size = size.saturating_add(u64::try_from(chunk.len())?);
        ensure!(size <= limit, "hosted response exceeds size limit");
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

async fn read_json(response: Response) -> Result<Value> {
    let file = tempfile::NamedTempFile::new()?;
    stage(response, file.path(), 1024 * 1024).await?;
    serde_json::from_reader(file.as_file()).context("invalid hosted sharding response")
}
