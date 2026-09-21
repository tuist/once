//! Graph capability commands for build, lint, run, and test.
//!
//! This module owns command orchestration: resolving a target from the
//! workspace graph, checking the requested capability, executing actions
//! declared by target kinds or generic fallback actions, and rendering the result. The legacy
//! capability fallback lives in [`action`].

mod action;
mod analysis;
mod build_receipt;
mod capability;
mod configuration;
mod contract;
mod lint;

pub use contract::{validate_action_contracts, ActionContractValidation};

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{EvidenceCacheState, LintSeverity, ResourceLimits, RunEventBus, SandboxMode};
use once_frontend::analysis::AnalysisOptions;
use once_frontend::GraphTarget;

pub use self::capability::load_graph_for_capability_with_configuration;
use self::capability::{
    build_target, ensure_capability, record_capability_run, run_target_capability, write_record,
    CapabilityRunRecord,
};
use self::lint::{
    lint_provider_output_path, persist_lint_results, validate_lint_provider, write_lint_results,
};
use crate::bus_events::{self, BusOutputObserver};
use crate::cli::{ColorChoice, Format, Output};
use crate::reporter::{ColorMode, ReporterOptions, TerminalReporter, Verbosity};

/// Capacity of the per-run event bus. Sized so a burst of per-target
/// phase and log events cannot make a slow subscriber drop its own
/// snapshot updates.
const EVENT_BUS_CAPACITY: usize = 1024;

fn spawn_reporter(
    bus: &RunEventBus,
    output: Output,
    command_label: &str,
) -> Option<TerminalReporter> {
    if output.format != Format::Human || output.quiet {
        return None;
    }
    let options = ReporterOptions {
        command_label: command_label.to_string(),
        initial_status: Some("loading graph".to_string()),
        color: match output.color {
            ColorChoice::Auto => ColorMode::Auto,
            ColorChoice::Always => ColorMode::Always,
            ColorChoice::Never => ColorMode::Never,
        },
        verbosity: match output.verbose {
            0 => Verbosity::Normal,
            1 => Verbosity::Verbose,
            _ => Verbosity::ExtraVerbose,
        },
        suppress_panel: false,
    };
    Some(TerminalReporter::spawn(bus, options))
}

pub(crate) use configuration::ResolvedConfiguration;

/// Parse `--config` override strings and merge them over the
/// workspace-declared configuration into the invocation-scoped
/// configuration that build, lint, run, and test consume.
pub(crate) fn resolve_invocation_configuration(
    workspace: &Path,
    overrides: &[String],
) -> Result<ResolvedConfiguration> {
    let parsed = configuration::parse_overrides(overrides)?;
    configuration::resolve(workspace, &parsed)
}

/// The earlier of two watcher positions, or `None` when they cannot be
/// compared because they came from different watcher instances.
fn earliest_position<'a>(
    left: Option<&'a crate::commands::change_tracker::ChangePosition>,
    right: Option<&'a crate::commands::change_tracker::ChangePosition>,
) -> Option<&'a crate::commands::change_tracker::ChangePosition> {
    match (left, right) {
        (Some(left), Some(right)) if left.instance_id == right.instance_id => {
            if left.source_generation.min(left.output_generation)
                <= right.source_generation.min(right.output_generation)
            {
                Some(left)
            } else {
                Some(right)
            }
        }
        // Different instances describe different histories, so neither window
        // covers the other and there is nothing to reuse.
        (Some(_), Some(_)) => None,
        (found, None) | (None, found) => found,
    }
}

#[derive(Debug, Clone, Default)]
pub struct GraphRunOptions {
    pub visible: bool,
    pub arguments: Vec<String>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn build(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
    resolved: &configuration::ResolvedConfiguration,
    ui: bool,
) -> Result<ExitCode> {
    let started_at = Instant::now();
    let bus = RunEventBus::new(EVENT_BUS_CAPACITY);
    let command_label = format!("build {target_id}");
    let reporter = spawn_reporter(&bus, output, &command_label);
    #[cfg(feature = "events-ingest")]
    let live_reporter = crate::live_run_reporter::spawn(
        &bus,
        workspace,
        once_core::Xdg::from_env(),
        cache_provider_account(workspace),
        cache_provider_project(workspace),
    )
    .await;
    bus_events::run_started(&bus, target_id, bus_events::now_ms());

    let ui_server = if ui {
        Some(crate::commands::ui::UiServer::start().await?)
    } else {
        None
    };
    if let Some(ui_server) = &ui_server {
        eprintln!("Runs interface: {}", ui_server.url());
    }
    let publisher = ui_server
        .as_ref()
        .map(crate::commands::ui::UiServer::publisher);
    let run_context = if ui {
        let workspace = workspace.to_path_buf();
        let target = target_id.to_string();
        let configuration = resolved.configuration.clone();
        Some(
            tokio::task::spawn_blocking(move || {
                crate::commands::ui::RunContext::build(&workspace, target, &configuration)
            })
            .await
            .context("preparing the Runs build graph")?,
        )
    } else {
        None
    };
    // Optional live event ingest to a compatible ingest server. Enabled
    // by ONCE_EVENTS_ENDPOINT; failures are logged and never abort the
    // run. Subscribing before publisher.started() ensures RunStarted
    // is captured. Gated behind the `events-ingest` build feature so
    // that self-hosted graph builds of the CLI do not require the
    // once-events-client crate.
    #[cfg(feature = "events-ingest")]
    let mut event_client = if let (Some(server), Some(context)) = (&ui_server, &run_context) {
        match crate::commands::events::try_start(server, context.run_id_string()).await {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%error, "event ingest disabled for this run");
                None
            }
        }
    } else {
        None
    };
    if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
        publisher.started(run_context).await;
        publisher
            .progress(run_context, "Preparing the Once build graph…\n")
            .await;
    }
    bus_events::target_cache_checking(&bus, target_id);
    let live_output = publisher
        .as_ref()
        .zip(run_context.as_ref())
        .map(|(publisher, run_context)| publisher.live_output(run_context));
    // If the UI dashboard is not attached, keep an observer that only
    // publishes LogChunk events onto the bus so the terminal reporter
    // (and the ingest client, when the feature is enabled) can still
    // render captured child output.
    let bus_observer: Option<std::sync::Arc<dyn once_core::ActionOutputObserver>> =
        if live_output.is_none() {
            Some(BusOutputObserver::new(bus.clone(), target_id.to_string()))
        } else {
            None
        };
    let xdg = once_core::Xdg::from_env();
    let stored_receipt =
        build_receipt::read(workspace, target_id, sandbox, &resolved.path_suffix).await;
    let receipt_position = stored_receipt.as_ref().map(build_receipt::position);
    // Two things want to know what moved and they were last written at
    // different moments: this target's receipt, and the workspace's recorded
    // digests. Ask about the window covering both, so one round trip serves
    // them. A window wider than either needs is only ever conservative.
    let digest_position = analysis::recorded_digest_position(workspace);
    let prior_position = earliest_position(receipt_position, digest_position.as_ref());
    let initial_snapshot =
        crate::commands::change_tracker::snapshot(workspace, &xdg, &[], prior_position).await;
    if let Some(record) = build_receipt::load(
        workspace,
        target_id,
        sandbox,
        &resolved.path_suffix,
        initial_snapshot.as_ref(),
        stored_receipt,
    )
    .await
    {
        let duration_ms: u64 = started_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
            publisher
                .progress(run_context, "Reused the previous Once build result.\n")
                .await;
            publisher
                .finished(
                    run_context,
                    &record.action_digest,
                    duration_ms,
                    &record.cache,
                    0,
                    None,
                )
                .await;
        }
        bus_events::target_finished(&bus, target_id, duration_ms, &record.cache, 0);
        write_record(output, &record).await?;
        write_runs_report(workspace, ui_server.as_ref()).await;
        #[cfg(feature = "events-ingest")]
        finish_live_reporter(live_reporter).await;
        finish_reporter(reporter).await;
        return Ok(ExitCode::SUCCESS);
    }
    let changes = analysis::known_changes(initial_snapshot.as_ref(), digest_position.as_ref());
    let mut session = match analysis::BuildSession::load_workspace_with_configuration(
        workspace, cache, sandbox, resolved, &changes,
    )
    .await
    {
        Ok(session) => {
            let session = session
                .with_resource_limits(resource_limits)
                .with_event_bus(bus.clone());
            match (&live_output, &bus_observer) {
                (Some(live_output), _) => session.with_output_observer(live_output.observer()),
                (None, Some(observer)) => session.with_output_observer(observer.clone()),
                (None, None) => session,
            }
        }
        Err(error) => {
            let duration_ms: u64 = started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
                publisher
                    .progress(run_context, &format!("Build setup failed: {error}\n"))
                    .await;
                publisher.failed(run_context, duration_ms).await;
            }
            bus_events::target_failed(&bus, target_id, duration_ms);
            write_runs_report(workspace, ui_server.as_ref()).await;
            #[cfg(feature = "events-ingest")]
            finish_live_reporter(live_reporter).await;
            finish_reporter(reporter).await;
            return Err(error);
        }
    };
    if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
        publisher
            .progress(
                run_context,
                "Analysing the Once targets and starting actions…\n",
            )
            .await;
    }
    bus_events::target_preparing(&bus, target_id);
    session.with_known_changes(changes);
    let session = session;
    let target = match session.target(target_id) {
        Ok(target) => target,
        Err(error) => {
            let duration_ms: u64 = started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            if let Some(live_output) = &live_output {
                live_output.flush().await;
            }
            if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
                publisher
                    .progress(run_context, &format!("Build setup failed: {error}\n"))
                    .await;
                publisher.failed(run_context, duration_ms).await;
            }
            bus_events::target_failed(&bus, target_id, duration_ms);
            write_runs_report(workspace, ui_server.as_ref()).await;
            #[cfg(feature = "events-ingest")]
            finish_live_reporter(live_reporter).await;
            finish_reporter(reporter).await;
            return Err(error);
        }
    };
    let record = match build_target(workspace, cache, target, &session, sandbox).await {
        Ok(record) => {
            if let Some(live_output) = &live_output {
                live_output.flush().await;
            }
            record
        }
        Err(error) => {
            let duration_ms: u64 = started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            if let Some(live_output) = &live_output {
                live_output.flush().await;
            }
            if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
                publisher
                    .progress(run_context, &format!("Build failed: {error}\n"))
                    .await;
                publisher.failed(run_context, duration_ms).await;
            }
            bus_events::target_failed(&bus, target_id, duration_ms);
            write_runs_report(workspace, ui_server.as_ref()).await;
            #[cfg(feature = "events-ingest")]
            finish_live_reporter(live_reporter).await;
            finish_reporter(reporter).await;
            return Err(error);
        }
    };
    record_capability_run(workspace, &record).await;
    // An uncacheable build (any declared action with `cacheable = false`, which
    // several target kinds such as Dockerfile, Go, and Android use) must run on
    // every invocation. Never persist a receipt for it, and drop any stale one,
    // so the fast path can never skip its mandatory work.
    if record.cache_state == EvidenceCacheState::Bypass {
        build_receipt::clear(workspace, target_id, sandbox, &resolved.path_suffix).await;
        let duration_ms: u64 = started_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
            publisher
                .finished(
                    run_context,
                    &record.action_digest,
                    duration_ms,
                    &record.cache,
                    record.result.exit_code,
                    None,
                )
                .await;
        }
        bus_events::target_finished(
            &bus,
            target_id,
            duration_ms,
            &record.cache,
            record.result.exit_code,
        );
        write_record(output, &record).await?;
        write_runs_report(workspace, ui_server.as_ref()).await;
        #[cfg(feature = "events-ingest")]
        finish_live_reporter(live_reporter).await;
        finish_reporter(reporter).await;
        return Ok(ExitCode::SUCCESS);
    }
    // Where the journal stands, not where it will stand once the platform has
    // finished telling the watcher about the outputs this build just wrote.
    // Waiting for that costs hundreds of milliseconds and buys nothing: the
    // next invocation recognises those writes as ours.
    let final_snapshot = crate::commands::change_tracker::position(
        workspace,
        &xdg,
        &record.outputs,
        initial_snapshot.as_ref().map(|snapshot| &snapshot.position),
    )
    .await;
    if let Some(final_snapshot) = final_snapshot {
        // The recorded digests now describe the workspace as of this position,
        // which is what the next invocation asks the watcher about.
        session.record_digest_position(Some(final_snapshot.position.clone()));
        session.record_resolution(Some(&final_snapshot.position));
        session.record_target_outcomes(Some(&final_snapshot.position));
        if initial_snapshot.as_ref().is_some_and(|initial| {
            initial.position.instance_id == final_snapshot.position.instance_id
                && (initial.position.source_generation == final_snapshot.position.source_generation
                    || session.source_changes_match(final_snapshot.source_changes.as_deref()))
        }) {
            let environment = session.observed_environment();
            let host_paths = session.observed_host_paths();
            let source_digests =
                session.observed_source_digests(final_snapshot.source_changes.as_deref());
            let produced = session.recorded_output_digests(&record.outputs);
            build_receipt::store(
                workspace,
                target_id,
                sandbox,
                &resolved.path_suffix,
                resolved.digest,
                &final_snapshot,
                build_receipt::Observations {
                    environment: &environment,
                    host_paths: &host_paths,
                    source_digests: &source_digests,
                    produced: &produced,
                },
                &record,
            )
            .await;
        }
    }
    let duration_ms: u64 = started_at
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    bus_events::target_capturing(&bus, target_id);
    bus_events::target_publishing(&bus, target_id);
    if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
        publisher
            .finished(
                run_context,
                &record.action_digest,
                duration_ms,
                &record.cache,
                record.result.exit_code,
                None,
            )
            .await;
    }
    // The scheduler already fired `TargetCompleted` for the top-level
    // target from `build_one`; publish only the outer `RunCompleted`
    // here to avoid a duplicate completion line.
    bus_events::run_completed(&bus, record.result.exit_code);
    write_record(output, &record).await?;
    #[cfg(feature = "events-ingest")]
    if let Some(handle) = event_client.take() {
        handle
            .shutdown_with_timeout(std::time::Duration::from_secs(3))
            .await;
    }
    write_runs_report(workspace, ui_server.as_ref()).await;
    emit_capability_completion_sounds(&record);
    #[cfg(feature = "events-ingest")]
    finish_live_reporter(live_reporter).await;
    finish_reporter(reporter).await;
    Ok(ExitCode::SUCCESS)
}

/// Await the terminal reporter's shutdown so its final summary lands
/// before the CLI returns. A no-op when no reporter was spawned.
async fn finish_reporter(reporter: Option<TerminalReporter>) {
    if let Some(reporter) = reporter {
        reporter.finish().await;
    }
}

/// Await the HTTP run reporter so any pending per-target invocation POST has
/// a chance to complete before the CLI returns. Best-effort - the reporter
/// Drain the live gRPC reporter. Idempotent on a `Some` value.
#[cfg(feature = "events-ingest")]
async fn finish_live_reporter(reporter: crate::live_run_reporter::LiveRunReporter) {
    reporter.finish().await;
}

/// The Tuist account this workspace's cache is configured against.
///
/// Read from the resolved cache provider config so the live URL and the
/// gRPC target the CLI prints match the credentials the build is using.
fn cache_provider_account(workspace: &std::path::Path) -> Option<String> {
    let xdg = once_core::Xdg::from_env();
    match crate::cache_provider::resolve_config(workspace, &xdg).ok()? {
        crate::cache_provider::ResolvedCacheProviderConfig::Tuist(config) => config.account,
        crate::cache_provider::ResolvedCacheProviderConfig::Local => None,
    }
}

/// The Tuist project this workspace's cache is configured against.
fn cache_provider_project(workspace: &std::path::Path) -> Option<String> {
    let xdg = once_core::Xdg::from_env();
    match crate::cache_provider::resolve_config(workspace, &xdg).ok()? {
        crate::cache_provider::ResolvedCacheProviderConfig::Tuist(config) => config.project,
        crate::cache_provider::ResolvedCacheProviderConfig::Local => None,
    }
}

fn emit_capability_completion_sounds(record: &CapabilityRunRecord) {
    let action_event = if record.result.exit_code != 0 {
        crate::sound::Event::ActionFailed
    } else if record.cache_state == EvidenceCacheState::Hit {
        crate::sound::Event::ActionCacheHit
    } else {
        crate::sound::Event::ActionExecuted
    };
    crate::sound::emit(action_event);
    crate::sound::emit(if record.result.exit_code == 0 {
        crate::sound::Event::Finished
    } else {
        crate::sound::Event::Failed
    });
}

async fn write_runs_report(workspace: &Path, ui_server: Option<&crate::commands::ui::UiServer>) {
    let Some(ui_server) = ui_server else {
        return;
    };
    match ui_server.write_static_site(workspace).await {
        Ok(Some(report)) => eprintln!("Runs report: {}", report.display()),
        Ok(None) => {}
        Err(error) => tracing::warn!(error = %error, "could not write the static Runs report"),
    }
}

/// Run static analysis for one target and report whether findings met
/// `fail_on` as a plain bool, so a caller that lints several targets in one
/// invocation can OR the outcomes together and translate the result into a
/// process exit code once.
#[allow(clippy::too_many_arguments)]
pub async fn lint_returning_fails(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    fail_on: LintSeverity,
    resource_limits: ResourceLimits,
    resolved: &configuration::ResolvedConfiguration,
) -> Result<bool> {
    let graph =
        once_frontend::load_graph_workspace_with_configuration(workspace, &resolved.configuration)
            .context("loading graph")?;
    let session = analysis::BuildSession::new_with_options_with_configuration(
        workspace,
        cache,
        graph,
        AnalysisOptions::default(),
        sandbox,
        resolved,
    )
    .await?
    .with_resource_limits(resource_limits);
    let target = session.target(target_id)?;
    let capability = ensure_capability(target, "lint")?;
    let outcome = session
        .run_with_analysis_and_provider_validation(target, "lint", validate_lint_provider)
        .await?
        .ok_or_else(|| anyhow!("{} does not implement its lint capability", target.label.id))?;
    let report_path = lint_provider_output_path(target, &outcome.provider, "sarif")?;
    let results_path = lint_provider_output_path(target, &outcome.provider, "results")?;
    let results = once_core::read_sarif_results(target.label.id.as_str(), report_path, workspace)?;
    persist_lint_results(workspace, results_path, &results).await?;
    let record = CapabilityRunRecord {
        target: target.label.id.clone(),
        kind: target.kind.clone(),
        capability: capability.name.clone(),
        status: "completed".to_string(),
        action_digest: outcome.action_digest.to_string(),
        cache: outcome.cache_tag.to_string(),
        output_groups: capability.output_groups.clone(),
        required_outputs: capability.requires_outputs.clone(),
        outputs: outcome.outputs,
        test_results: None,
        input_digest: outcome.input_digest,
        input_fingerprint: outcome.input_fingerprint,
        cache_state: outcome.cache_state,
        result: outcome.result,
    };
    record_capability_run(workspace, &record).await;
    write_lint_results(output, &results).await?;
    let fails = results.fails_at(fail_on);
    let action_event = if fails {
        crate::sound::Event::ActionFailed
    } else if record.cache_state == EvidenceCacheState::Hit {
        crate::sound::Event::ActionCacheHit
    } else {
        crate::sound::Event::ActionExecuted
    };
    crate::sound::emit(action_event);
    crate::sound::emit(if fails {
        crate::sound::Event::Failed
    } else {
        crate::sound::Event::Finished
    });
    Ok(fails)
}

#[allow(clippy::too_many_arguments)]
pub async fn test(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
    resolved: &configuration::ResolvedConfiguration,
    ui: bool,
) -> Result<ExitCode> {
    test_with_filters(
        workspace,
        cache,
        output,
        target_id,
        sandbox,
        &[],
        None,
        resource_limits,
        resolved,
        ui,
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn test_with_filters(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    test_filters: &[String],
    test_batch_id: Option<&str>,
    resource_limits: ResourceLimits,
    resolved: &configuration::ResolvedConfiguration,
    ui: bool,
) -> Result<ExitCode> {
    if let Some(batch_id) = test_batch_id {
        if Digest::from_hex(batch_id).is_none() {
            anyhow::bail!("invalid internal test batch identifier");
        }
    }
    let started_at = Instant::now();
    let bus = RunEventBus::new(EVENT_BUS_CAPACITY);
    let command_label = format!("test {target_id}");
    let reporter = spawn_reporter(&bus, output, &command_label);
    #[cfg(feature = "events-ingest")]
    let live_reporter = crate::live_run_reporter::spawn(
        &bus,
        workspace,
        once_core::Xdg::from_env(),
        cache_provider_account(workspace),
        cache_provider_project(workspace),
    )
    .await;
    bus_events::run_started(&bus, target_id, bus_events::now_ms());

    let ui_server = if ui {
        Some(crate::commands::ui::UiServer::start().await?)
    } else {
        None
    };
    if let Some(ui_server) = &ui_server {
        eprintln!("Runs interface: {}", ui_server.url());
    }
    let publisher = ui_server
        .as_ref()
        .map(crate::commands::ui::UiServer::publisher);
    let run_context = if ui {
        let workspace = workspace.to_path_buf();
        let target = target_id.to_string();
        let configuration = resolved.configuration.clone();
        Some(
            tokio::task::spawn_blocking(move || {
                crate::commands::ui::RunContext::test(&workspace, target, &configuration)
            })
            .await
            .context("preparing the Runs test graph")?,
        )
    } else {
        None
    };
    if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
        publisher.started(run_context).await;
        publisher
            .progress(run_context, "Preparing the Once test run…\n")
            .await;
    }
    bus_events::target_cache_checking(&bus, target_id);
    let live_output = publisher
        .as_ref()
        .zip(run_context.as_ref())
        .map(|(publisher, run_context)| publisher.live_output(run_context));
    let bus_observer: Option<std::sync::Arc<dyn once_core::ActionOutputObserver>> =
        if live_output.is_none() {
            Some(BusOutputObserver::new(bus.clone(), target_id.to_string()))
        } else {
            None
        };
    let graph = match once_frontend::load_graph_workspace_with_configuration(
        workspace,
        &resolved.configuration,
    ) {
        Ok(graph) => graph,
        Err(error) => {
            let duration_ms: u64 = started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
                publisher
                    .progress(run_context, &format!("Test setup failed: {error}\n"))
                    .await;
                publisher.failed(run_context, duration_ms).await;
            }
            bus_events::target_failed(&bus, target_id, duration_ms);
            write_runs_report(workspace, ui_server.as_ref()).await;
            #[cfg(feature = "events-ingest")]
            finish_live_reporter(live_reporter).await;
            finish_reporter(reporter).await;
            return Err(error).context("loading graph");
        }
    };
    if !test_filters.is_empty() {
        let manifest =
            crate::commands::query::test_manifest_record_with_graph(workspace, target_id, &graph)?;
        for test_filter in test_filters {
            crate::commands::query::validate_test_unit(&manifest, target_id, test_filter)?;
        }
    }
    let session = analysis::BuildSession::new_with_options_with_configuration(
        workspace,
        cache,
        graph,
        AnalysisOptions {
            test_filters: test_filters.to_vec(),
            test_batch_id: test_batch_id.map(str::to_string),
            ..AnalysisOptions::default()
        },
        sandbox,
        resolved,
    )
    .await?
    .with_resource_limits(resource_limits)
    .with_event_bus(bus.clone());
    let session = match (&live_output, &bus_observer) {
        (Some(live_output), _) => session.with_output_observer(live_output.observer()),
        (None, Some(observer)) => session.with_output_observer(observer.clone()),
        (None, None) => session,
    };
    if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
        publisher
            .progress(run_context, "Running the selected Once test target…\n")
            .await;
    }
    bus_events::target_preparing(&bus, target_id);
    bus_events::target_executing(&bus, target_id);
    let target = session.target(target_id)?;
    let test_capability = ensure_capability(target, "test")?;
    if !test_capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|capability| capability.name == "build")
    {
        let build_record = build_target(workspace, cache, target, &session, sandbox).await?;
        record_capability_run(workspace, &build_record).await;
    }
    let record = if let Some(outcome) = session.run_with_analysis(target, "test").await? {
        let analysis::BuildOutcome {
            provider,
            action_digest,
            input_digest,
            input_fingerprint,
            outputs,
            cache_tag,
            cache_state,
            result,
            ..
        } = outcome;
        let test_results = provider
            .pointer("/test_info/outputs/results")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        CapabilityRunRecord {
            target: target.label.id.clone(),
            kind: target.kind.clone(),
            capability: test_capability.name.clone(),
            status: "completed".to_string(),
            action_digest: action_digest.to_string(),
            cache: cache_tag.to_string(),
            output_groups: test_capability.output_groups.clone(),
            required_outputs: test_capability.requires_outputs.clone(),
            outputs,
            test_results,
            input_digest,
            input_fingerprint,
            cache_state,
            result,
        }
    } else {
        run_target_capability(workspace, cache, target, "test", sandbox).await?
    };
    if let Some(live_output) = &live_output {
        live_output.flush().await;
    }
    record_capability_run(workspace, &record).await;
    if test_filters.is_empty() {
        crate::commands::query::refresh_test_manifest_for_target(workspace, target)
            .context("persisting test manifest")?;
    }
    let test_results =
        load_test_results_for_runs(workspace, target_id, record.test_results.as_deref());
    // Fan the normalized test-results blob out as
    // `TestSuiteStarted` / `TestCaseCompleted` / `TestSuiteCompleted`
    // events on the bus so the server projector can persist per-case
    // rows. These cases are retrospective because this runner supplies a
    // report only after execution. Streaming-capable runners publish live
    // starts and completions directly from their output observer.
    tracing::debug!(
        has_test_results = test_results.is_some(),
        target = target_id,
        "publishing test results"
    );
    if let Some(results) = &test_results {
        let n = results
            .get("cases")
            .and_then(serde_json::Value::as_array)
            .map_or(0, std::vec::Vec::len);
        tracing::debug!(cases = n, target = target_id, "publishing test cases");
        publish_test_results_events(&bus, target_id, results);
    }
    let duration_ms: u64 = started_at
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    bus_events::target_capturing(&bus, target_id);
    bus_events::target_publishing(&bus, target_id);
    if let (Some(publisher), Some(run_context)) = (&publisher, &run_context) {
        publisher
            .finished(
                run_context,
                &record.action_digest,
                duration_ms,
                &record.cache,
                record.result.exit_code,
                test_results,
            )
            .await;
    }
    // The scheduler already fired `TargetCompleted` for the top-level
    // target from `build_one`; publish only the outer `RunCompleted`
    // here to avoid a duplicate completion line.
    bus_events::run_completed(&bus, record.result.exit_code);
    write_record(output, &record).await?;
    write_runs_report(workspace, ui_server.as_ref()).await;
    emit_capability_completion_sounds(&record);
    #[cfg(feature = "events-ingest")]
    finish_live_reporter(live_reporter).await;
    finish_reporter(reporter).await;
    Ok(ExitCode::SUCCESS)
}

// Fan the normalized `once.test_results.v1` blob out onto the event
// bus. Emits one `TestSuiteStarted` (with the total case count),
// one `TestCaseCompleted` per attempt of each case, and a closing
// `TestSuiteCompleted` with the aggregated totals.
//
// Retrospective results carry their own case, suite, and attempt identity.
// They do not invent a start time when the test runner only exposes a report.
fn publish_test_results_events(
    bus: &once_core::RunEventBus,
    target_id: &str,
    results: &serde_json::Value,
) {
    use once_core::{RunEvent, TestCaseResult, TestTotals};

    let now = bus_events::now_ms();

    let cases = results.get("cases").and_then(serde_json::Value::as_array);

    let case_count = cases.map_or(0, std::vec::Vec::len);

    bus.publish(RunEvent::TestSuiteStarted {
        at_epoch_ms: now,
        target_id: target_id.to_string(),
        planned_case_count: Some(u32::try_from(case_count).unwrap_or(u32::MAX)),
    });

    let mut totals = TestTotals {
        unknown: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        errored: 0,
        timed_out: 0,
        cancelled: 0,
    };

    if let Some(cases) = cases {
        for case in cases {
            let case_id = case
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let name = case
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(case_id);
            let suite_id = case
                .get("suite")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let attempts = case.get("attempts").and_then(serde_json::Value::as_array);
            if attempts.is_none() || case_id.is_empty() {
                continue;
            }
            for (idx, attempt) in attempts.unwrap().iter().enumerate() {
                let status = attempt
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("passed");
                let result = match status {
                    "failed" | "fail" | "failure" => TestCaseResult::Failed,
                    "skipped" | "ignored" | "pending" => TestCaseResult::Skipped,
                    "errored" | "error" => TestCaseResult::Errored,
                    "timed_out" | "timeout" => TestCaseResult::TimedOut,
                    "cancelled" | "canceled" => TestCaseResult::Cancelled,
                    "passed" | "pass" | "ok" => TestCaseResult::Passed,
                    _ => TestCaseResult::Unknown,
                };
                let duration_ms = attempt
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let duration_known = attempt
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_i64)
                    .is_some();
                let failure_message = attempt
                    .get("failure")
                    .and_then(|v| v.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                match result {
                    TestCaseResult::Unknown => totals.unknown += 1,
                    TestCaseResult::Passed => totals.passed += 1,
                    TestCaseResult::Failed => totals.failed += 1,
                    TestCaseResult::Skipped => totals.skipped += 1,
                    TestCaseResult::Errored => totals.errored += 1,
                    TestCaseResult::TimedOut => totals.timed_out += 1,
                    TestCaseResult::Cancelled => totals.cancelled += 1,
                }
                bus.publish(RunEvent::TestCaseCompleted {
                    at_epoch_ms: bus_events::now_ms(),
                    target_id: target_id.to_string(),
                    case_id: case_id.to_string(),
                    name: name.to_string(),
                    suite_id: suite_id.to_string(),
                    attempt: u32::try_from(idx + 1).unwrap_or(u32::MAX),
                    result,
                    duration_ms,
                    duration_known,
                    failure_message,
                });
            }
        }
    }

    bus.publish(RunEvent::TestSuiteCompleted {
        at_epoch_ms: bus_events::now_ms(),
        target_id: target_id.to_string(),
        totals,
    });
}

fn load_test_results_for_runs(
    workspace: &Path,
    target_id: &str,
    result_path: Option<&str>,
) -> Option<serde_json::Value> {
    let result = match result_path {
        Some(result_path) => crate::commands::query::test_results_value_at(
            workspace,
            target_id,
            Some(result_path),
            &[],
        ),
        None => crate::commands::query::test_results_value(workspace, target_id),
    };
    result
        .inspect_err(|error| {
            tracing::debug!(error = %error, target = target_id, "could not load test results for Runs");
        })
        .ok()
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    workspace: &Path,
    cache: &CacheProvider,
    graph: Vec<GraphTarget>,
    output: Output,
    target_id: &str,
    options: GraphRunOptions,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
    resolved: &configuration::ResolvedConfiguration,
) -> Result<ExitCode> {
    let session = analysis::BuildSession::new_with_options_with_configuration(
        workspace,
        cache,
        graph,
        AnalysisOptions {
            run_visible: options.visible,
            run_arguments: options.arguments,
            ..AnalysisOptions::default()
        },
        sandbox,
        resolved,
    )
    .await?
    .with_resource_limits(resource_limits);
    let target = session.target(target_id)?;
    let run_capability = ensure_capability(target, "run")?;
    if !run_capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|capability| capability.name == "build")
    {
        let build_record = build_target(workspace, cache, target, &session, sandbox).await?;
        record_capability_run(workspace, &build_record).await;
    }
    let record = if let Some(outcome) = session.run_with_analysis(target, "run").await? {
        let analysis::BuildOutcome {
            action_digest,
            input_digest,
            input_fingerprint,
            outputs,
            cache_tag,
            cache_state,
            result,
            ..
        } = outcome;
        CapabilityRunRecord {
            target: target.label.id.clone(),
            kind: target.kind.clone(),
            capability: run_capability.name.clone(),
            status: "completed".to_string(),
            action_digest: action_digest.to_string(),
            cache: cache_tag.to_string(),
            output_groups: run_capability.output_groups.clone(),
            required_outputs: run_capability.requires_outputs.clone(),
            outputs,
            test_results: None,
            input_digest,
            input_fingerprint,
            cache_state,
            result,
        }
    } else {
        run_target_capability(workspace, cache, target, "run", sandbox).await?
    };
    record_capability_run(workspace, &record).await;
    write_record(output, &record).await?;
    emit_capability_completion_sounds(&record);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::capability::{graph_supports, render_human};
    use super::*;
    use once_cas::ActionResult;
    use once_frontend::{Capability, TargetLabel};

    #[test]
    fn retrospective_cases_preserve_unknown_status_and_attempt_identity() {
        let bus = once_core::RunEventBus::new(16);
        let mut receiver = bus.subscribe();
        let results = serde_json::json!({
            "cases": [{
                "id": "parser::case",
                "name": "case",
                "suite": "parser",
                "attempts": [{"status": "unknown"}, {"status": "passed"}]
            }]
        });
        publish_test_results_events(&bus, "tests", &results);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            once_core::RunEvent::TestSuiteStarted {
                planned_case_count: Some(1),
                ..
            }
        ));
        assert!(
            matches!(receiver.try_recv().unwrap(), once_core::RunEvent::TestCaseCompleted {
            case_id, name, suite_id, attempt: 1, result: once_core::TestCaseResult::Unknown, ..
        } if case_id == "parser::case" && name == "case" && suite_id == "parser")
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            once_core::RunEvent::TestCaseCompleted {
                attempt: 2,
                result: once_core::TestCaseResult::Passed,
                ..
            }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            once_core::RunEvent::TestSuiteCompleted {
                totals: once_core::TestTotals {
                    unknown: 1,
                    passed: 1,
                    ..
                },
                ..
            }
        ));
    }

    fn action_result() -> ActionResult {
        ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        }
    }

    fn graph_target(kind: &str, capabilities: &[&str]) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: "apps/ios".to_string(),
                name: "App".to_string(),
                id: "apps/ios/App".to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: capabilities
                .iter()
                .map(|name| Capability {
                    name: (*name).to_string(),
                    output_groups: Vec::new(),
                    requires_outputs: Vec::new(),
                })
                .collect(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn lint_provider_requires_structured_output_paths() {
        let target = graph_target("custom_lint", &["lint"]);
        let provider = serde_json::json!({
            "lint_info": {
                "outputs": {
                    "sarif": ".once/out/lint/report.sarif"
                }
            }
        });

        let error = validate_lint_provider(&target, &provider).unwrap_err();
        let failure = error
            .downcast_ref::<once_frontend::analysis::AnalysisFailure>()
            .expect("structured analysis failure");

        assert_eq!(failure.diagnostic.code, "invalid_lint_provider_output");
        assert_eq!(failure.diagnostic.target.as_deref(), Some("apps/ios/App"));
        assert_eq!(
            failure.diagnostic.attribute.as_deref(),
            Some("lint_info.outputs.results")
        );
        assert!(!failure.diagnostic.repairs.is_empty());
    }

    #[test]
    fn ensure_capability_returns_matching_capability() {
        let target = graph_target("custom_application", &["build", "run"]);
        let capability = ensure_capability(&target, "run").unwrap();
        assert_eq!(capability.name, "run");
    }

    #[test]
    fn graph_supports_finds_a_capability_in_a_preloaded_graph() {
        let graph = vec![graph_target("custom_application", &["build", "run"])];

        assert!(graph_supports(&graph, "apps/ios/App", "run"));
        assert!(!graph_supports(&graph, "apps/ios/App", "test"));
        assert!(!graph_supports(&graph, "apps/ios/Missing", "run"));
    }

    #[test]
    fn unsupported_capability_lists_available_capabilities() {
        let target = graph_target("custom_application", &["build", "run"]);
        let err = ensure_capability(&target, "test").unwrap_err().to_string();
        assert!(err.contains("does not expose `test`"));
        assert!(err.contains("Available capabilities: build, run"));
    }

    #[test]
    fn unsupported_capability_reports_when_none_declared() {
        let target = graph_target("mystery", &[]);
        let err = ensure_capability(&target, "build").unwrap_err().to_string();
        assert!(err.contains("does not expose any capabilities"));
    }

    #[test]
    fn render_human_includes_requires_and_paths() {
        let record = CapabilityRunRecord {
            target: "apps/ios/App".to_string(),
            kind: "custom_application".to_string(),
            capability: "run".to_string(),
            status: "completed".to_string(),
            action_digest: "deadbeef".to_string(),
            cache: "miss".to_string(),
            output_groups: vec!["default".to_string()],
            required_outputs: vec!["bundle".to_string()],
            outputs: vec![".once/out/apps/ios/App/run".to_string()],
            test_results: None,
            input_digest: None,
            input_fingerprint: None,
            cache_state: EvidenceCacheState::Miss,
            result: action_result(),
        };

        let rendered = render_human(&record);

        assert!(rendered.contains("once: run apps/ios/App (custom_application) cache miss, exit=0"));
        assert!(rendered.contains("outputs: default"));
        assert!(rendered.contains("requires: bundle"));
        assert!(rendered.contains("  .once/out/apps/ios/App/run"));
    }

    #[test]
    fn render_human_reports_no_output_groups() {
        let record = CapabilityRunRecord {
            target: "apps/ios/App".to_string(),
            kind: "custom_application".to_string(),
            capability: "build".to_string(),
            status: "completed".to_string(),
            action_digest: "deadbeef".to_string(),
            cache: "hit".to_string(),
            output_groups: Vec::new(),
            required_outputs: Vec::new(),
            outputs: Vec::new(),
            test_results: None,
            input_digest: None,
            input_fingerprint: None,
            cache_state: EvidenceCacheState::Hit,
            result: action_result(),
        };

        let rendered = render_human(&record);

        assert!(rendered.contains("outputs: none"));
        assert!(!rendered.contains("requires:"));
        assert!(!rendered.contains("paths:"));
    }
}
