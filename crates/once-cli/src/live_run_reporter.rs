//! Streams the internal run event bus to the discovered event service.
//!
//! Subscription happens before the first run event. A bounded preflight
//! resolves the endpoint and authenticates before graph work starts, while
//! a dedicated runtime keeps synchronous graph preparation from blocking
//! network progress. Finishing the reporter drains queued action completions
//! and the terminal run event before the process exits.
//!
//! While a run streams, SIGINT and SIGTERM complete it as cancelled: the
//! first signal stops the event stream, publishes a cancelled `RunCompleted`,
//! drains for a short bounded time, and then ends the process as the signal
//! would have. A second signal ends it immediately. Runs that never reach the
//! event service keep the default signal behavior.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use once_cas::{TuistAuth, TuistCacheConfig, TUIST_OAUTH_CLIENT_ID_ENV};
use once_core::{RunEventBus, Xdg};
use once_events_client::{
    CredentialsError, EventClient, ReconnectPolicy, RunCancellation, SessionLimits,
    TransportConfig, TransportError,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::argv_normalize::SafeContext;
use crate::cache_provider::{credentials_root, resolve_config, ResolvedCacheProviderConfig};
use crate::discovery;
use crate::termination::{self, SignalWatcher};

/// How long a cancelled run may spend delivering its terminal event. Well
/// inside the grace period CI runners give between the first signal and a kill.
const CANCEL_DRAIN: Duration = Duration::from_secs(2);

/// Hard ceiling on the signal path, covering transport work outside the
/// cancel drain such as a reconnect in progress.
const CANCEL_DEADLINE: Duration = Duration::from_secs(3);

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
                tracing::warn!(%error, "live run reporter finished with error");
                eprintln!("Once insights could not be fully delivered: {error}");
            }
            Err(error) => {
                tracing::warn!(%error, "live run reporter task ended abnormally");
                eprintln!("Once insights reporter stopped unexpectedly");
            }
        }
    }
}

/// Establish reporting with a bounded preflight, then stream in the background.
/// Missing configuration or a connection failure is logged without failing
/// the build.
#[allow(clippy::too_many_lines)]
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

            let Some(endpoint_url) = resolve_events(&workspace, &xdg).await else {
                tracing::debug!("Once live reporter: no events endpoint configured");
                return Ok(0);
            };

            let Some(token) = auth_token(&workspace, &xdg) else {
                tracing::debug!("Once live reporter: no credentials configured");
                return Ok(0);
            };

            let channel = match build_channel(&endpoint_url).await {
                Ok(channel) => channel,
                Err(error) => {
                    tracing::debug!(
                        error = %error_chain(&error),
                        "Once live reporter: gRPC connect failed"
                    );
                    return Ok(0);
                }
            };

            let config = TransportConfig {
                run_id: run_id.clone(),
                batch_flush: Duration::from_millis(150),
                cancel_drain: CANCEL_DRAIN,
                limits: SessionLimits::default(),
                ..TransportConfig::default()
            };
            let project_id = tuist_project_id(&account, &project);
            let client = match project_id.as_deref() {
                Some(project_id) => {
                    EventClient::authenticated_with_project_id(channel, config, &token, project_id)
                }
                None => EventClient::authenticated(channel, config, &token)
                    .map_err(CredentialsError::from),
            };
            let mut client = match client {
                Ok(client) => client,
                Err(CredentialsError::Token(_)) => {
                    tracing::debug!("Once live reporter: invalid authorization metadata");
                    return Ok(0);
                }
                Err(CredentialsError::ProjectId) => {
                    tracing::debug!("Once live reporter: project id is not valid metadata");
                    return Ok(0);
                }
            };
            let caps = tokio::time::timeout(Duration::from_secs(5), client.capabilities())
                .await
                .map_err(|_| TransportError::PreflightTimeout)??;
            let allowlist_version = match caps.safe_literal_allowlist_version.as_str() {
                crate::argv_normalize::SAFE_LITERAL_ALLOWLIST_VERSION => {
                    crate::argv_normalize::SAFE_LITERAL_ALLOWLIST_VERSION
                }
                crate::argv_normalize::BASE_SAFE_LITERAL_ALLOWLIST_VERSION => {
                    crate::argv_normalize::BASE_SAFE_LITERAL_ALLOWLIST_VERSION
                }
                _ => return Err(TransportError::UnsupportedCapabilities),
            };
            let key = tokio::time::timeout(
                Duration::from_secs(5),
                client.argv_hash_key(project_id.clone().unwrap_or_default()),
            )
            .await
            .map_err(|_| TransportError::PreflightTimeout)??;
            let mut argv: Vec<String> = std::env::args().collect();
            if let Some(command) = argv.first_mut() {
                *command = "once".to_string();
            }
            let git_rev = git_revision(&workspace);
            let safe_context =
                if allowlist_version == crate::argv_normalize::SAFE_LITERAL_ALLOWLIST_VERSION {
                    build_safe_context(&workspace)
                } else {
                    SafeContext::empty()
                };
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
                safe_literal_allowlist_version: allowlist_version.to_string(),
                project_id: project_id.unwrap_or_default(),
                is_ci: once_events_client::environment::is_ci(),
                ..Default::default()
            };

            let signals = SignalWatcher::install();
            let cancellation = RunCancellation::new();
            let _ = ready_tx.send(());
            let renewal_workspace = workspace.clone();
            let renewal_xdg = xdg.clone();
            let transport = client
                .with_metadata(metadata)
                .with_cancellation(cancellation.clone())
                .with_token_provider(move || {
                    let workspace = renewal_workspace.clone();
                    let xdg = renewal_xdg.clone();
                    async move {
                        tokio::task::spawn_blocking(move || auth_token(&workspace, &xdg))
                            .await
                            .ok()
                            .flatten()
                    }
                })
                .with_dashboard_link(|link| eprintln!("\n  ↗ Once live: {link}\n"))
                .run_with_reconnect(bus_rx, shutdown_rx, ReconnectPolicy::default());
            match signals {
                Some(signals) => stream_until_signal(transport, signals, &cancellation).await,
                None => transport.await,
            }
        })
    });

    let _ = tokio::time::timeout(Duration::from_secs(12), ready_rx).await;
    LiveRunReporter {
        handle,
        shutdown: Some(shutdown_tx),
        system_sampler: Some(system_sampler),
    }
}

/// Drive the transport while watching for termination signals. On the first
/// signal the run is cancelled and given [`CANCEL_DEADLINE`] to deliver its
/// terminal event before the process ends by that signal. A second signal
/// ends the process from the signal handler without waiting. When the
/// transport finishes first, signals go back to their default behavior.
async fn stream_until_signal(
    transport: impl std::future::Future<Output = Result<u64, TransportError>>,
    mut signals: SignalWatcher,
    cancellation: &RunCancellation,
) -> Result<u64, TransportError> {
    tokio::pin!(transport);
    let signal = tokio::select! {
        result = &mut transport => {
            if let Some(signal) = signals.restore() {
                termination::terminate(signal);
            }
            return result;
        }
        signal = signals.recv() => signal,
    };
    cancellation.cancel(signal.name());
    let name = signal.name();
    off_the_signal_path(move || {
        tracing::info!(
            signal = name,
            "run cancelled by signal; reporting it before exiting"
        );
        let _ = writeln!(
            std::io::stderr(),
            "\nOnce received {name}; reporting the cancelled run before exiting."
        );
    });
    // The transport logs its own delivery outcome; the process ends either way.
    let _ = tokio::time::timeout(CANCEL_DEADLINE, &mut transport).await;
    termination::terminate(signal)
}

/// Run `report` on its own thread. Logging and the user notice write to
/// stderr synchronously, and a blocked stderr must not hold up the drain or
/// the exit.
fn off_the_signal_path(report: impl FnOnce() + Send + 'static) {
    std::thread::spawn(report);
}

/// The project id this provider's server expects. It is the only place the
/// `account/project` shape lives; the transport and the protocol treat the id
/// as opaque.
fn tuist_project_id(account: &str, project: &str) -> Option<String> {
    if account.is_empty() || project.is_empty() {
        return None;
    }
    Some(format!("{account}/{project}"))
}

async fn resolve_events(workspace: &Path, xdg: &Xdg) -> Option<String> {
    let ResolvedCacheProviderConfig::Tuist(config) = resolve_config(workspace, xdg).ok()? else {
        return None;
    };

    let base = config.url.trim_end_matches('/').to_string();
    match discovery::fetch(&base).await {
        Ok(Some(doc)) => {
            let url = doc.events.first()?.clone();
            Some(url)
        }
        _ => None,
    }
}

pub(crate) async fn build_channel(url: &str) -> Result<Channel, tonic::transport::Error> {
    let mut endpoint =
        Endpoint::from_shared(normalize_grpc_url(url))?.connect_timeout(Duration::from_secs(5));
    // The parsed URI lowercases the scheme, so `HTTPS://` is matched too.
    if endpoint.uri().scheme_str() == Some("https") {
        endpoint = endpoint.tls_config(ClientTlsConfig::new().with_enabled_roots())?;
    }
    endpoint.connect().await
}

// tonic reports a failed connect as a bare "transport error" and keeps the
// reason in the source chain, so log every cause instead of only the outermost.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
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
/// Reads target labels only when the workspace explicitly opts in with
/// `[reporting] argv_privacy = "workspace"` in root `once.toml`.
/// Unknown or unreadable configuration retains strict redaction.
fn build_safe_context(workspace: &Path) -> SafeContext {
    if !workspace_disclosure_enabled(workspace) {
        return SafeContext::empty();
    }
    let mut context = SafeContext::empty();
    if let Ok(targets) = once_frontend::load_graph_workspace(workspace) {
        for target in targets {
            context.extend([target.label.id.clone(), target.label.name.clone()]);
        }
    }
    context
}

fn workspace_disclosure_enabled(workspace: &Path) -> bool {
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
        .is_some_and(|mode| mode.eq_ignore_ascii_case("workspace"))
}

fn new_run_id() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let non_negative = u64::try_from(now_ms).unwrap_or(u64::MAX);
    let uuid = uuid::Uuid::new_v7(uuid::Timestamp::from_unix(
        uuid::NoContext,
        non_negative / 1000,
        u32::try_from((non_negative % 1000) * 1_000_000).unwrap_or(0),
    ));
    format!("run-{uuid}")
}

fn git_revision(workspace: &Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use super::{build_channel, error_chain, tuist_project_id, workspace_disclosure_enabled};

    #[test]
    fn the_project_id_needs_both_halves() {
        assert_eq!(
            tuist_project_id("tuist", "once").as_deref(),
            Some("tuist/once")
        );
        assert_eq!(tuist_project_id("tuist", ""), None);
        assert_eq!(tuist_project_id("", "once"), None);
        assert_eq!(tuist_project_id("", ""), None);
    }

    // Dials a plain TCP listener through `build_channel` and returns the first
    // byte the client sent. Both waits are bounded so a client that never dials
    // fails the test instead of hanging it.
    async fn first_byte_sent_by(url: impl FnOnce(u16) -> String) -> Option<u8> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let first_byte = tokio::spawn(async move {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .ok()?
                .ok()?;
            let mut byte = [0_u8; 1];
            tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut byte))
                .await
                .ok()?
                .ok()?;
            Some(byte[0])
        });

        let _ = build_channel(&url(port)).await;

        first_byte.await.unwrap()
    }

    // 0x16 is the TLS handshake record type, the first byte of a ClientHello.
    const TLS_HANDSHAKE: u8 = 0x16;

    // Without a TLS config the channel gave up right after the TCP connect and
    // never sent a byte, so `once` could not stream to any `grpcs://` server.
    #[tokio::test]
    async fn grpcs_endpoints_open_with_a_tls_handshake() {
        let first = first_byte_sent_by(|port| format!("grpcs://127.0.0.1:{port}")).await;
        assert_eq!(first, Some(TLS_HANDSHAKE));
    }

    #[tokio::test]
    async fn the_https_scheme_is_matched_case_insensitively() {
        let first = first_byte_sent_by(|port| format!("HTTPS://127.0.0.1:{port}")).await;
        assert_eq!(first, Some(TLS_HANDSHAKE));
    }

    // A plaintext HTTP/2 client opens with the connection preface, which starts
    // with `P`, so local `grpc://` servers keep working without TLS.
    #[tokio::test]
    async fn grpc_endpoints_stay_plaintext() {
        let first = first_byte_sent_by(|port| format!("grpc://127.0.0.1:{port}")).await;
        assert_eq!(first, Some(b'P'));
    }

    #[derive(Debug)]
    struct Layered(&'static str, Option<Box<Layered>>);

    impl std::fmt::Display for Layered {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl std::error::Error for Layered {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1
                .as_deref()
                .map(|cause| cause as &(dyn std::error::Error + 'static))
        }
    }

    // tonic reports connect failures as a bare "transport error" and keeps the
    // reason in the source chain, which is what hid the missing TLS config.
    #[test]
    fn error_chain_includes_every_cause() {
        let error = Layered(
            "transport error",
            Some(Box::new(Layered(
                "tls handshake",
                Some(Box::new(Layered("unknown issuer", None))),
            ))),
        );

        assert_eq!(
            error_chain(&error),
            "transport error: tls handshake: unknown issuer"
        );
    }

    #[test]
    fn argument_disclosure_requires_explicit_workspace_setting() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(!workspace_disclosure_enabled(workspace.path()));
        std::fs::write(
            workspace.path().join("once.toml"),
            "[reporting]\nargv_privacy = 'strict'\n",
        )
        .unwrap();
        assert!(!workspace_disclosure_enabled(workspace.path()));
        std::fs::write(
            workspace.path().join("once.toml"),
            "[reporting]\nargv_privacy = 'workspace'\n",
        )
        .unwrap();
        assert!(workspace_disclosure_enabled(workspace.path()));
    }
}
