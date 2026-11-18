use std::path::Path;

use anyhow::{bail, Context, Result};
use struo_core::artifacts::{ArtifactExport, PublicationIntent};
use struo_core::Xdg;
use struo_frontend::{InfrastructureProviderConfig, NamedCacheProviderConfig};

use super::{service, tuist, Publication};

pub(super) async fn publish(
    workspace: &Path,
    package: &ArtifactExport,
    publication: &Publication,
    binding: NamedCacheProviderConfig,
    provider: InfrastructureProviderConfig,
    automatic: bool,
    allow_on_build: bool,
) -> Result<Option<String>> {
    match provider {
        InfrastructureProviderConfig::Local => Ok(Some(
            reqwest::Url::from_directory_path(&package.directory)
                .map_err(|()| anyhow::anyhow!("artifact export directory must be absolute"))?
                .to_string(),
        )),
        InfrastructureProviderConfig::Tuist(config) => {
            let config = struo_cas::TuistCacheConfig {
                url: config.url,
                account: binding.account.or(config.account),
                project: binding.project.or(config.project),
                oauth_client_id: config.oauth_client_id,
                provider_name: publication.provider.clone(),
            };
            let xdg = Xdg::from_env();
            let endpoint = tuist::endpoint(&config)?;
            let authorization =
                service::authorization_path(workspace, &publication.target, &endpoint, &xdg)?;
            if automatic && !authorization.is_file() {
                tracing::warn!(target = %publication.target, "automatic publication requires explicit destination authorization");
                return Ok(None);
            }
            let approved = if automatic {
                Some(tokio::fs::read_to_string(&authorization).await?)
            } else {
                None
            };
            let service = tuist::connect(&config, &xdg).await?;
            if let Some(approved) = approved {
                service.require_destination(&approved)?;
            }
            // The Tuist server matches this legacy `once` schema name.
            let intent = PublicationIntent {
                schema: "once.publication_intent.v1".into(),
                root: package.root.clone(),
                target: publication.target.clone(),
                track: publication.track.clone(),
                revision: publication.git_commit.clone(),
            };
            let receipt = service.publish(package, &intent).await?;
            if allow_on_build {
                let parent = authorization
                    .parent()
                    .context("invalid authorization path")?;
                tokio::fs::create_dir_all(parent).await?;
                let temporary = tempfile::NamedTempFile::new_in(parent)?;
                tokio::fs::write(temporary.path(), receipt.destination.as_bytes()).await?;
                temporary.persist(authorization)?;
            }
            Ok(Some(receipt.url))
        }
        _ => bail!("selected infrastructure does not support artifact publication"),
    }
}
