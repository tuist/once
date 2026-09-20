//! Streams the internal run event bus to the discovered event service.
//!
//! Subscription happens before the first run event. A bounded preflight
//! resolves the endpoint and authenticates before graph work starts, while
//! a dedicated runtime keeps synchronous graph preparation from blocking
//! network progress. Finishing the reporter drains queued action completions
//! and the terminal run event before the process exits.

#![cfg(feature = "events-ingest")]

use std::path::Path;
use std::time::Duration;

use once_cas::{TUIST_OAUTH_CLIENT_ID_ENV, TuistAuth, TuistCacheConfig};
use once_core::{RunEventBus, Xdg};
use once_events_client::{
    EventClient, ReconnectPolicy, SessionLimits, TransportConfig, TransportError,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::transport::{Channel, Endpoint};

use crate::argv_normalize::SafeContext;
use crate::cache_provider::{ResolvedCacheProviderConfig, credentials_root, resolve_config};
use crate::discovery;

/// Handle to a spawned live reporter. Await [`Self::finish`] before the
/// process exits so the terminal `RunCompleted` batch has time to drain.
pub struct LiveRunReporter {
    handle: JoinHandle<Result<u64, TransportError>>,
    system_sampler: Option<crate::bus_events::SystemSamplerHandle>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl LiveRunReporter {
    /// Signal the reporter that the run is done and wait for it to
    /// flush. Idempotent; safe to call once per spawn.
    pub async fn finish(mut self) {
        if let Some(sampler) = self.system_sampler.take() {
            sampler.stop().await;
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        match self.handle.await {
            Ok(Ok(seq)) => {
                tracing::debug!(final_seq = seq, "live run reporter drained");
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "live run reporter finished with error");
            }
            Err(error) => {
                tracing::debug!(%error, "live run reporter task ended abnormally");
            }
        }
    }
}

/// Establish reporting with a bounded preflight, then stream in the background.
/// Missing configuration or a connection failure is logged without failing
/// the build.
pub async fn spawn(
    bus: &RunEventBus,
    workspace: &Path,
    xdg: Xdg,
    account: Option<String>,
    project: Option<String>,
) -> LiveRunReporter {
    let workspace = workspace.to_path_buf();
    let bus_rx = bus.subscribe();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let system_sampler = crate::bus_events::spawn_system_sampler(bus);

    let handle = tokio::task::spawn_blocking(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            tracing::debug!("Once live reporter: could not start event runtime");
            return Ok(0);
        };
        runtime.block_on(async move {
            let run_id = new_run_id();
            let account = account.unwrap_or_default();
            let project = project.unwrap_or_default();

            let (endpoint_url, live_url_template) = match resolve_events(&workspace, &xdg).await {
                Some(resolved) => resolved,
                None => {
                    tracing::debug!("Once live reporter: no events endpoint configured");
                    return Ok(0);
                }
            };

            let Some(token) = auth_token(&workspace, &xdg) else {
                tracing::debug!("Once live reporter: no credentials configured");
                return Ok(0);
            };

            let channel = match build_channel(&endpoint_url).await {
                Ok(channel) => channel,
                Err(error) => {
                    tracing::debug!(%error, "Once live reporter: gRPC connect failed");
                    return Ok(0);
                }
            };

            // Print the live URL as soon as we have it. Best-effort: skip
            // when the template is missing (the server did not advertise
            // one) rather than fail the build.
            if let Some(template) = live_url_template.as_deref() {
                let live_url = template
                    .replace("{account}", &account)
                    .replace("{project}", &project)
                    .replace("{run_id}", &run_id);
                eprintln!("\n  ↗ Once live: {live_url}\n");
            }

            let client = EventClient::authenticated(
                channel,
                TransportConfig {
                    run_id: run_id.clone(),
                    batch_flush: Duration::from_millis(150),
                    final_drain: Duration::from_secs(2),
                    limits: SessionLimits::default(),
                },
                &token,
            );
            let Ok(mut client) = client else {
                tracing::debug!("Once live reporter: invalid authorization metadata");
                return Ok(0);
            };
            let key = client.argv_hash_key(format!("{account}/{project}")).await?;
            let mut argv: Vec<String> = std::env::args().collect();
            if let Some(command) = argv.first_mut() {
                *command = "once".to_string();
            }
            let git_rev = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&workspace)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_default();
            let safe_context = build_safe_context(&workspace);
            let metadata = once_events_client::proto::RunStarted {
                // `ONCE_VERSION` is stamped by `crates/once-cli/build.rs`
                // from `ONCE_RELEASE_VERSION` at release time and
                // falls back to the crate version for local builds,
                // so this always reads the version the operator
                // shipped rather than the workspace placeholder.
                once_version: env!("ONCE_VERSION").to_string(),
                protocol_version: "1.0".to_string(),
                host_class: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                git_rev,
                argv_normalized: crate::argv_normalize::normalize_argv_with_context(
                    &argv,
                    &key.key_bytes,
                    &safe_context,
                ),
                argv_hash_key_id: key.key_id,
                cwd_relative: crate::argv_normalize::cwd_relative(&workspace, &workspace),
                safe_literal_allowlist_version:
                    crate::argv_normalize::SAFE_LITERAL_ALLOWLIST_VERSION.to_string(),
                project_id: format!("{account}/{project}"),
                ..Default::default()
            };

            let _ = ready_tx.send(());
            client
                .with_metadata(metadata)
                .run_with_reconnect(bus_rx, shutdown_rx, ReconnectPolicy::default())
                .await
        })
    });

    let _ = tokio::time::timeout(Duration::from_secs(12), ready_rx).await;
    LiveRunReporter {
        handle,
        shutdown: Some(shutdown_tx),
        system_sampler: Some(system_sampler),
    }
}

async fn resolve_events(workspace: &Path, xdg: &Xdg) -> Option<(String, Option<String>)> {
    let ResolvedCacheProviderConfig::Tuist(config) = resolve_config(workspace, xdg).ok()? else {
        return None;
    };

    let base = config.url.trim_end_matches('/').to_string();
    match discovery::fetch(&base).await {
        Ok(Some(doc)) => {
            let url = doc.events.first_url()?.to_string();
            Some((url, doc.live_url_template))
        }
        _ => None,
    }
}

async fn build_channel(url: &str) -> Result<Channel, tonic::transport::Error> {
    let endpoint = Endpoint::from_shared(normalize_grpc_url(url))?;
    endpoint.connect().await
}

// gRPC targets in `/.well-known/once` are advertised as
// `grpcs://build.tuist.dev` / `grpc://localhost:4001`. tonic accepts
// `https://` / `http://` in the URL, so translate the scheme here rather
// than at the discovery layer (keeping discovery honest to the RFC).
fn normalize_grpc_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("grpcs://") {
        format!("https://{rest}")
    } else if let Some(rest) = url.strip_prefix("grpc://") {
        format!("http://{rest}")
    } else {
        url.to_string()
    }
}

fn auth_token(workspace: &Path, xdg: &Xdg) -> Option<String> {
    let ResolvedCacheProviderConfig::Tuist(config) = resolve_config(workspace, xdg).ok()? else {
        return None;
    };
    read_token(&config, xdg)
}

fn read_token(config: &TuistCacheConfig, xdg: &Xdg) -> Option<String> {
    let auth = TuistAuth::new(credentials_root(xdg), config);
    let _ = std::env::var(TUIST_OAUTH_CLIENT_ID_ENV);
    auth.token().ok()
}

/// Build the workspace safe context for RFC 0009 argv classification.
///
/// Reads the loaded graph's target labels and honours a workspace
/// opt-out. `[reporting] argv_privacy = "strict"` in root `once.toml`
/// returns an empty context, which reproduces the RFC 0008 behaviour
/// verbatim. Any failure to load the graph or read the manifest also
/// returns an empty context: widening classification is a
/// best-effort readability improvement, never a correctness
/// guarantee.
fn build_safe_context(workspace: &Path) -> SafeContext {
    if is_strict_mode(workspace) {
        return SafeContext::empty();
    }
    let mut context = SafeContext::empty();
    if let Ok(targets) = once_frontend::load_graph_workspace(workspace) {
        for target in targets {
            context.insert(target.label.id.clone());
            context.insert(target.label.name.clone());
        }
    }
    context
}

fn is_strict_mode(workspace: &Path) -> bool {
    let manifest_path = workspace.join("once.toml");
    let Ok(source) = std::fs::read_to_string(&manifest_path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&source) else {
        return false;
    };
    value
        .get("reporting")
        .and_then(|table| table.get("argv_privacy"))
        .and_then(|v| v.as_str())
        .is_some_and(|mode| mode.eq_ignore_ascii_case("strict"))
}

fn new_run_id() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let non_negative = now_ms.min(u128::from(u64::MAX)) as u64;
    let uuid = uuid::Uuid::new_v7(uuid::Timestamp::from_unix(
        uuid::NoContext,
        non_negative / 1000,
        u32::try_from((non_negative % 1000) * 1_000_000).unwrap_or(0),
    ));
    format!("run-{uuid}")
}
