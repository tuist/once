use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_core::{
    ActionOutputObserver, ActionOutputStream, LogStream, RunEvent, RunEventBus,
};
use once_frontend::{AttrValue, BuildConfiguration};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

mod assets;
mod server;

pub use server::UiServer;

use server::RunStore;

#[derive(Clone)]
pub struct Publisher {
    store: RunStore,
}

#[derive(Clone)]
pub struct RunContext {
    run_id: String,
    workspace: String,
    target: String,
    display_target: String,
    operation: RunOperation,
    started_at_ms: u64,
    graph: Option<BuildGraph>,
}

#[derive(Clone, Copy)]
enum RunOperation {
    Build,
    Test,
}

#[derive(Clone, Serialize)]
struct RunSnapshot {
    run_id: String,
    action_digest: String,
    workspace: String,
    target: String,
    display_target: String,
    operation: String,
    command: String,
    status: String,
    started_at_ms: u64,
    duration_ms: Option<u64>,
    cache: Option<String>,
    exit_code: Option<i32>,
    graph: Option<BuildGraph>,
    test_results: Option<serde_json::Value>,
    logs: Vec<OutputRecord>,
    output_truncated: bool,
    static_report_path: Option<String>,
    #[serde(skip)]
    output_byte_count: usize,
}

#[derive(Clone, Serialize)]
struct OutputRecord {
    stream: String,
    text: String,
    at_ms: u64,
}

#[derive(Clone, Serialize)]
pub(super) struct BuildGraph {
    target_count: usize,
    resolved_target_count: usize,
    nodes: Vec<BuildGraphNode>,
    #[serde(skip)]
    display_target: Option<String>,
}

#[derive(Clone, Serialize)]
struct BuildGraphNode {
    id: String,
    name: String,
    package: String,
    kind: String,
    deps: Vec<String>,
    build_target: bool,
}

const OUTPUT_CHANNEL_CAPACITY: usize = 64;
const OUTPUT_LOG_LIMIT: usize = 500;
const OUTPUT_BYTE_LIMIT: usize = 65_536;

pub struct LiveOutput {
    observer: Arc<OutputObserver>,
}

struct OutputObserver {
    sender: mpsc::Sender<OutputMessage>,
    dropped_output: AtomicBool,
    decoder: Mutex<OutputDecoder>,
    event_bus: Option<RunEventBus>,
    target_id: Option<String>,
}

enum OutputMessage {
    Chunk {
        stream: ActionOutputStream,
        text: String,
    },
    Flush {
        done: oneshot::Sender<()>,
        dropped_output: bool,
    },
}

#[derive(Default)]
struct OutputDecoder {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl OutputObserver {
    fn new(
        sender: mpsc::Sender<OutputMessage>,
        event_bus: Option<RunEventBus>,
        target_id: Option<String>,
    ) -> Self {
        Self {
            sender,
            dropped_output: AtomicBool::new(false),
            decoder: Mutex::new(OutputDecoder::default()),
            event_bus,
            target_id,
        }
    }

    fn publish_chunk(&self, stream: ActionOutputStream, bytes: &[u8]) {
        let Some(bus) = &self.event_bus else { return };
        let Some(target_id) = &self.target_id else {
            return;
        };
        bus.publish(RunEvent::LogChunk {
            at_epoch_ms: now_epoch_ms(),
            target_id: target_id.clone(),
            stream: match stream {
                ActionOutputStream::Stdout => LogStream::Stdout,
                ActionOutputStream::Stderr => LogStream::Stderr,
            },
            bytes: bytes.to_vec(),
        });
    }

    fn queue(&self, stream: ActionOutputStream, text: String) {
        if text.is_empty() {
            return;
        }
        if matches!(
            self.sender.try_send(OutputMessage::Chunk { stream, text }),
            Err(mpsc::error::TrySendError::Full(_))
        ) {
            self.dropped_output.store(true, Ordering::Release);
        }
    }

    fn flush_decoder(&self) {
        let pending = self
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish();
        for (stream, text) in pending {
            self.queue(stream, text);
        }
    }

    fn take_dropped_output(&self) -> bool {
        self.dropped_output.swap(false, Ordering::AcqRel)
    }
}

impl OutputDecoder {
    fn append(&mut self, stream: ActionOutputStream, bytes: &[u8]) -> Option<String> {
        let buffered = match stream {
            ActionOutputStream::Stdout => &mut self.stdout,
            ActionOutputStream::Stderr => &mut self.stderr,
        };
        buffered.extend_from_slice(bytes);
        let text = match std::str::from_utf8(buffered) {
            Ok(text) => {
                let text = text.to_string();
                buffered.clear();
                text
            }
            Err(error) if error.error_len().is_none() => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to == 0 {
                    return None;
                }
                let remainder = buffered.split_off(valid_up_to);
                let text = std::str::from_utf8(buffered)
                    .map(str::to_string)
                    .unwrap_or_default();
                *buffered = remainder;
                text
            }
            Err(_) => {
                let text = String::from_utf8_lossy(buffered).into_owned();
                buffered.clear();
                text
            }
        };
        Some(text)
    }

    fn finish(&mut self) -> Vec<(ActionOutputStream, String)> {
        [
            (ActionOutputStream::Stdout, std::mem::take(&mut self.stdout)),
            (ActionOutputStream::Stderr, std::mem::take(&mut self.stderr)),
        ]
        .into_iter()
        .filter_map(|(stream, bytes)| {
            (!bytes.is_empty()).then(|| (stream, String::from_utf8_lossy(&bytes).into_owned()))
        })
        .collect()
    }
}

impl ActionOutputObserver for OutputObserver {
    fn observe(&self, stream: ActionOutputStream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.publish_chunk(stream, bytes);
        let text = self
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append(stream, bytes);
        if let Some(text) = text {
            self.queue(stream, text);
        }
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

impl Publisher {
    #[must_use]
    fn from_store(store: RunStore) -> Self {
        Self { store }
    }

    fn event_bus(&self) -> RunEventBus {
        self.store.event_bus()
    }

    /// Announce that the run started, seeding the UI snapshot store.
    /// Bus emission for the lifecycle event lives in
    /// [`crate::bus_events`] so a run without the UI still fires it.
    #[allow(clippy::unused_async)]
    pub async fn started(&self, context: &RunContext) {
        self.store.replace(RunSnapshot {
            run_id: context.run_id.clone(),
            action_digest: "pending".to_string(),
            workspace: context.workspace.clone(),
            target: context.target.clone(),
            display_target: context.display_target.clone(),
            operation: context.operation.label().to_string(),
            command: command_label(context.operation, &context.target),
            status: "running".to_string(),
            started_at_ms: context.started_at_ms,
            duration_ms: None,
            cache: None,
            exit_code: None,
            graph: context.graph.clone(),
            test_results: None,
            logs: Vec::new(),
            output_truncated: false,
            static_report_path: None,
            output_byte_count: 0,
        });
    }

    /// Update the UI snapshot with the run's terminal state. Bus
    /// emission of `TargetCompleted` and `RunCompleted` lives in
    /// [`crate::bus_events`] so it fires whether or not the UI is on.
    #[allow(clippy::unused_async)]
    pub async fn finished(
        &self,
        _context: &RunContext,
        action_digest: &str,
        duration_ms: u64,
        cache: &str,
        exit_code: i32,
        test_results: Option<serde_json::Value>,
    ) {
        let status = if exit_code == 0 {
            "completed"
        } else {
            "failed"
        };
        self.store.update(|run| {
            run.action_digest = action_digest.to_string();
            run.status = status.to_string();
            run.duration_ms = Some(duration_ms);
            run.cache = Some(cache.to_string());
            run.exit_code = Some(exit_code);
            run.test_results = test_results;
        });
    }

    /// Update the UI snapshot with a failure that terminated before the
    /// action reached the runner. Bus emission of the corresponding
    /// terminal events lives in [`crate::bus_events`].
    #[allow(clippy::unused_async)]
    pub async fn failed(&self, _context: &RunContext, duration_ms: u64) {
        self.store.update(|run| {
            run.status = "failed".to_string();
            run.duration_ms = Some(duration_ms);
            run.exit_code = Some(1);
        });
    }

    #[must_use]
    pub fn live_output(&self, context: &RunContext) -> LiveOutput {
        let (sender, mut receiver) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
        let store = self.store.clone();
        tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                match message {
                    OutputMessage::Chunk { stream, text } => {
                        append_output(&store, output_stream_name(stream), text);
                    }
                    OutputMessage::Flush {
                        done,
                        dropped_output,
                    } => {
                        if dropped_output {
                            mark_output_truncated(&store);
                        }
                        let _ = done.send(());
                    }
                }
            }
        });
        LiveOutput {
            observer: Arc::new(OutputObserver::new(
                sender,
                Some(self.event_bus()),
                Some(context.target.clone()),
            )),
        }
    }

    #[allow(clippy::unused_async)]
    pub async fn progress(&self, _context: &RunContext, message: &str) {
        append_output(&self.store, "notice", message.to_string());
    }
}

impl LiveOutput {
    #[must_use]
    pub fn observer(&self) -> Arc<dyn ActionOutputObserver> {
        self.observer.clone()
    }

    pub async fn flush(&self) {
        self.observer.flush_decoder();
        let dropped_output = self.observer.take_dropped_output();
        let (done, waiting) = oneshot::channel();
        if self
            .observer
            .sender
            .send(OutputMessage::Flush {
                done,
                dropped_output,
            })
            .await
            .is_ok()
        {
            let _ = waiting.await;
        }
    }
}

impl RunContext {
    /// Copy of the run id as an owned string. Used by the optional
    /// event ingest path to name the run on the wire.
    #[must_use]
    pub fn run_id_string(&self) -> String {
        self.run_id.clone()
    }

    #[must_use]
    pub fn build(workspace: &Path, target: String, configuration: &BuildConfiguration) -> Self {
        Self::new(workspace, target, configuration, RunOperation::Build)
    }

    #[must_use]
    pub fn test(workspace: &Path, target: String, configuration: &BuildConfiguration) -> Self {
        Self::new(workspace, target, configuration, RunOperation::Test)
    }

    fn new(
        workspace: &Path,
        target: String,
        configuration: &BuildConfiguration,
        operation: RunOperation,
    ) -> Self {
        let graph = BuildGraph::load(workspace, &target, configuration);
        let display_target = graph
            .as_ref()
            .and_then(BuildGraph::display_target)
            .unwrap_or_else(|| target.clone());
        Self {
            run_id: uuid::Uuid::now_v7().to_string(),
            workspace: workspace_label(workspace),
            graph,
            target,
            display_target,
            operation,
            started_at_ms: milliseconds(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default(),
            ),
        }
    }
}

impl RunOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
        }
    }
}

impl BuildGraph {
    fn load(workspace: &Path, target_id: &str, configuration: &BuildConfiguration) -> Option<Self> {
        let resolved = match once_frontend::load_graph_workspace_with_configuration(
            workspace,
            configuration,
        ) {
            Ok(targets) => targets,
            Err(error) => {
                tracing::debug!(error = %error, "could not resolve Once build graph for Runs");
                return None;
            }
        };
        let display_target = resolved
            .iter()
            .find(|target| target.label.id == target_id)
            .filter(|target| target.kind == "swift_package_workspace")
            .and_then(|target| match target.attrs.get("package_name") {
                Some(AttrValue::String(name)) if !name.is_empty() => Some(name.clone()),
                _ => None,
            });
        let resolved_by_id = resolved
            .iter()
            .map(|target| (target.label.id.as_str(), target))
            .collect::<BTreeMap<_, _>>();
        let mut reachable = BTreeSet::new();
        collect_reachable(target_id, &resolved_by_id, &mut reachable);
        if reachable.is_empty() {
            return None;
        }
        let nodes = reachable
            .iter()
            .filter_map(|id| {
                let target = resolved_by_id.get(id.as_str())?;
                let deps = target
                    .dependency_ids()
                    .filter(|dependency| reachable.contains(*dependency))
                    .cloned()
                    .collect::<Vec<_>>();
                Some(BuildGraphNode {
                    id: target.label.id.clone(),
                    name: target.label.name.clone(),
                    package: target.label.package.clone(),
                    kind: target.kind.clone(),
                    deps,
                    build_target: target.label.id == target_id,
                })
            })
            .collect::<Vec<_>>();
        if nodes.is_empty() {
            return None;
        }
        Some(Self {
            target_count: nodes.len(),
            resolved_target_count: reachable.len(),
            nodes,
            display_target,
        })
    }

    fn display_target(&self) -> Option<String> {
        self.display_target.clone()
    }
}

fn collect_reachable(
    target_id: &str,
    targets: &BTreeMap<&str, &once_frontend::GraphTarget>,
    reachable: &mut BTreeSet<String>,
) {
    if !reachable.insert(target_id.to_string()) {
        return;
    }
    let Some(target) = targets.get(target_id) else {
        return;
    };
    for dependency in target.dependency_ids() {
        collect_reachable(dependency, targets, reachable);
    }
}

fn workspace_label(workspace: &Path) -> String {
    workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace")
        .to_string()
}

fn command_label(operation: RunOperation, target: &str) -> String {
    format!("once {} {target}", operation.label())
}

fn append_output(store: &RunStore, stream: &str, text: String) {
    store.update(|run| {
        let was_truncated = text.len() > OUTPUT_BYTE_LIMIT;
        let text = truncate_to_byte_limit(text, OUTPUT_BYTE_LIMIT);
        run.output_byte_count = run.output_byte_count.saturating_add(text.len());
        run.logs.push(OutputRecord {
            stream: stream.to_string(),
            text,
            at_ms: milliseconds(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default(),
            ),
        });
        run.output_truncated |= was_truncated;
        while run.logs.len() > OUTPUT_LOG_LIMIT || run.output_byte_count > OUTPUT_BYTE_LIMIT {
            let removed = run.logs.remove(0);
            run.output_byte_count = run.output_byte_count.saturating_sub(removed.text.len());
            run.output_truncated = true;
        }
    });
}

fn mark_output_truncated(store: &RunStore) {
    store.update(|run| run.output_truncated = true);
}

fn truncate_to_byte_limit(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

fn output_stream_name(stream: ActionOutputStream) -> &'static str {
    match stream {
        ActionOutputStream::Stdout => "stdout",
        ActionOutputStream::Stderr => "stderr",
    }
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::Ordering,
        time::{SystemTime, UNIX_EPOCH},
    };

    use once_core::{ActionOutputObserver, ActionOutputStream};
    use tokio::sync::mpsc;

    use super::{
        append_output, OutputMessage, OutputObserver, RunContext, RunOperation, RunSnapshot,
        RunStore, UiServer, OUTPUT_BYTE_LIMIT,
    };

    #[cfg(target_os = "macos")]
    use super::BuildGraph;
    #[cfg(target_os = "macos")]
    use once_frontend::BuildConfiguration;
    #[cfg(target_os = "macos")]
    use std::fs;
    #[cfg(target_os = "macos")]
    use tempfile::TempDir;

    fn running_snapshot() -> RunSnapshot {
        RunSnapshot {
            run_id: "run-1".to_string(),
            action_digest: "pending".to_string(),
            workspace: "once".to_string(),
            target: "target".to_string(),
            display_target: "target".to_string(),
            operation: "build".to_string(),
            command: "once build target".to_string(),
            status: "running".to_string(),
            started_at_ms: 0,
            duration_ms: None,
            cache: None,
            exit_code: None,
            graph: None,
            test_results: None,
            logs: Vec::new(),
            output_truncated: false,
            static_report_path: None,
            output_byte_count: 0,
        }
    }

    #[test]
    fn retains_split_utf8_output() {
        let (sender, mut receiver) = mpsc::channel(1);
        let observer = OutputObserver::new(sender, None, None);

        observer.observe(ActionOutputStream::Stdout, &[0xe2, 0x80]);
        assert!(receiver.try_recv().is_err());
        observer.observe(ActionOutputStream::Stdout, &[0xa6]);

        match receiver.try_recv().unwrap() {
            OutputMessage::Chunk { stream, text } => {
                assert_eq!(stream, ActionOutputStream::Stdout);
                assert_eq!(text, "…");
            }
            OutputMessage::Flush { .. } => panic!("expected output chunk"),
        }
    }

    #[test]
    fn marks_dropped_output_when_the_channel_is_full() {
        let (sender, _receiver) = mpsc::channel(1);
        let observer = OutputObserver::new(sender, None, None);

        observer.observe(ActionOutputStream::Stdout, b"first");
        observer.observe(ActionOutputStream::Stdout, b"second");

        assert!(observer.dropped_output.load(Ordering::Acquire));
    }

    #[test]
    fn truncates_retained_output_at_a_character_boundary() {
        let store = RunStore::new();
        store.replace(running_snapshot());

        append_output(&store, "stdout", "é".repeat(OUTPUT_BYTE_LIMIT));
        append_output(&store, "stdout", "next".to_string());

        let run = store.latest().unwrap();
        assert_eq!(run.logs.len(), 1);
        assert_eq!(run.logs[0].text, "next");
        assert_eq!(run.output_byte_count, "next".len());
        assert!(run.output_truncated);
    }

    #[tokio::test]
    async fn publishes_build_updates_to_the_local_server() {
        let server = UiServer::start().await.unwrap();
        let publisher = server.publisher();
        let context = RunContext {
            run_id: "run-1".to_string(),
            workspace: "once".to_string(),
            target: "crates/once-cli/once".to_string(),
            display_target: "once".to_string(),
            operation: RunOperation::Build,
            started_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .try_into()
                .unwrap(),
            graph: None,
        };

        publisher.started(&context).await;
        publisher.progress(&context, "Starting build\n").await;
        publisher
            .finished(&context, "aabbcc", 42, "hit", 0, None)
            .await;

        let endpoint = server.url().replace("/runs/overview", "/api/runs/latest");
        let body: serde_json::Value = reqwest::get(endpoint).await.unwrap().json().await.unwrap();

        assert_eq!(body["status"], "completed");
        assert_eq!(body["cache"], "hit");
        assert_eq!(body["operation"], "build");
        assert_eq!(body["logs"][0]["text"], "Starting build\n");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn includes_reachable_resolved_targets_for_native_swift_packages() {
        let workspace = TempDir::new().unwrap();
        fs::create_dir_all(workspace.path().join("Sources/CModule/include")).unwrap();
        fs::create_dir_all(workspace.path().join("Sources/App")).unwrap();
        fs::write(
            workspace.path().join("Package.swift"),
            r#"// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CModuleImport",
    products: [.library(name: "CModuleImport", targets: ["App"])],
    targets: [
        .target(name: "CModule"),
        .target(name: "App", dependencies: ["CModule"]),
    ]
)
"#,
        )
        .unwrap();
        fs::write(
            workspace.path().join("Sources/CModule/module.c"),
            "int answer(void) { return 42; }\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("Sources/CModule/include/module.h"),
            "int answer(void);\n",
        )
        .unwrap();
        fs::write(
            workspace
                .path()
                .join("Sources/CModule/include/module.modulemap"),
            "module CModule { header \"module.h\" export * }\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("Sources/App/App.swift"),
            "import CModule\npublic let value = answer()\n",
        )
        .unwrap();

        let graph = BuildGraph::load(
            workspace.path(),
            "swift_package",
            &BuildConfiguration::default(),
        )
        .expect("native Swift package graph");

        assert_eq!(graph.target_count, 3);
        assert_eq!(graph.resolved_target_count, 3);
        assert_eq!(graph.display_target(), Some("CModuleImport".to_string()));
        assert!(graph
            .nodes
            .iter()
            .any(|node| { node.id == "SwiftPackage_CModuleImport_CModule" && !node.build_target }));
        let app = graph
            .nodes
            .iter()
            .find(|node| node.id == "SwiftPackage_CModuleImport_App")
            .expect("app target");
        assert_eq!(app.deps, vec!["SwiftPackage_CModuleImport_CModule"]);
    }
}
