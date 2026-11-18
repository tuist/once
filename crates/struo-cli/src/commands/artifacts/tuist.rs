use anyhow::{ensure, Context, Result};
use reqwest::Url;
use struo_cas::{TuistAuth, TuistCacheConfig};
use struo_core::Xdg;

use super::service::Service;

pub(super) async fn connect(config: &TuistCacheConfig, xdg: &Xdg) -> Result<Service> {
    let base = endpoint(config)?;
    let auth = TuistAuth::new(crate::cache_provider::credentials_root(xdg), config);
    let token = tokio::task::spawn_blocking(move || auth.token()).await??;
    Service::connect(base, token).await
}

pub(super) fn endpoint(config: &TuistCacheConfig) -> Result<Url> {
    let account = config
        .account
        .as_deref()
        .context("artifact infrastructure requires an account")?;
    let project = config
        .project
        .as_deref()
        .context("artifact infrastructure requires a project")?;
    let mut base = Url::parse(&format!("{}/", config.url.trim_end_matches('/')))?;
    ensure!(
        base.scheme() == "https"
            || base
                .host_str()
                .is_some_and(|host| host == "localhost" || host == "127.0.0.1"),
        "artifact publication requires HTTPS"
    );
    ensure!(
        base.username().is_empty()
            && base.password().is_none()
            && base.query().is_none()
            && base.fragment().is_none(),
        "artifact provider URL must not contain credentials, a query or a fragment"
    );
    base.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("invalid artifact provider URL"))?
        .pop_if_empty()
        .extend(["api", "projects", account, project, "artifacts"]);
    Ok(base)
}
