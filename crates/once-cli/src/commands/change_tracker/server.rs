use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use once_cas::Digest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::protocol::{Request, Response};
use super::{ChangePosition, ChangeSnapshot};

const MAX_JOURNAL_RECORDS: usize = 16_384;

/// How long a barrier waits for the watcher to report its own fence file.
///
/// A barrier writes a sentinel into a watched directory and waits to be
/// told about it. Because watch backends deliver events in order, seeing
/// the sentinel proves every earlier event has already landed in the
/// journal. That makes the wait bounded by the backend's coalescing
/// latency, which on macOS `FSEvents` reaches several seconds when the
/// machine is busy, so the budget is generous rather than tight: timing
/// out costs a caller its incremental fast path, and the default used to
/// be short enough to lose that path under load.
const DEFAULT_FENCE_TIMEOUT: Duration = Duration::from_secs(9);

/// How many fence sentinels one barrier will try before giving up. The
/// budget above is split evenly across them.
const FENCE_ATTEMPTS: u32 = 3;

/// Override for [`DEFAULT_FENCE_TIMEOUT`], in whole seconds. Lets a
/// heavily loaded or slow-filesystem host buy more headroom without a
/// rebuild.
const FENCE_TIMEOUT_ENV: &str = "ONCE_CHANGE_TRACKER_FENCE_TIMEOUT_SECS";

/// Effective fence wait, honouring [`FENCE_TIMEOUT_ENV`].
pub(super) fn fence_timeout() -> Duration {
    std::env::var(FENCE_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(DEFAULT_FENCE_TIMEOUT, Duration::from_secs)
}

struct ChangeRecord {
    generation: u64,
    paths: Option<BTreeSet<String>>,
}

#[derive(Default)]
struct ChangeJournal {
    source: VecDeque<ChangeRecord>,
    output: VecDeque<ChangeRecord>,
}

struct TrackerState {
    workspace: PathBuf,
    instance_id: String,
    source_generation: AtomicU64,
    output_generation: AtomicU64,
    journal: Mutex<ChangeJournal>,
    fences: Mutex<BTreeMap<String, oneshot::Sender<()>>>,
    source_watcher: Mutex<Option<RecommendedWatcher>>,
    watched_outputs: Mutex<BTreeMap<PathBuf, OutputIdentity>>,
    root_fingerprint: Mutex<Digest>,
}

pub(super) async fn serve(workspace: &Path, socket: &Path) -> Result<()> {
    let workspace = std::fs::canonicalize(workspace).context("resolving tracked workspace")?;
    let root_fingerprint = root_fingerprint(&workspace)?;
    let state = Arc::new(TrackerState {
        workspace,
        instance_id: Uuid::now_v7().to_string(),
        source_generation: AtomicU64::new(0),
        output_generation: AtomicU64::new(0),
        journal: Mutex::new(ChangeJournal::default()),
        fences: Mutex::new(BTreeMap::new()),
        source_watcher: Mutex::new(None),
        watched_outputs: Mutex::new(BTreeMap::new()),
        root_fingerprint: Mutex::new(root_fingerprint),
    });
    let callback_state = Arc::clone(&state);
    let mut source_watcher = notify::recommended_watcher(move |event| {
        callback_state.handle_event(event);
    })
    .context("creating filesystem change tracker")?;
    watch_sources(&mut source_watcher, &state.workspace)?;
    *state
        .source_watcher
        .lock()
        .expect("change tracker watcher lock poisoned") = Some(source_watcher);
    let listener = bind_listener(socket).await?;
    let mut workspace_check = tokio::time::interval(Duration::from_millis(100));
    workspace_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    workspace_check.tick().await;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accepting tracker client")?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, state).await {
                        tracing::debug!(%error, "filesystem change tracker client failed");
                    }
                });
            }
            _ = workspace_check.tick() => {
                if !state.workspace.exists() {
                    break;
                }
            }
        }
    }
    drop(listener);
    let _ = tokio::fs::remove_file(socket).await;
    Ok(())
}

async fn bind_listener(socket: &Path) -> Result<UnixListener> {
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating change tracker directory {}", parent.display()))?;
    }
    if tokio::fs::try_exists(socket).await.unwrap_or(false) {
        if UnixStream::connect(socket).await.is_ok() {
            anyhow::bail!("filesystem change tracker is already running");
        }
        tokio::fs::remove_file(socket)
            .await
            .with_context(|| format!("removing stale tracker socket {}", socket.display()))?;
    }
    UnixListener::bind(socket)
        .with_context(|| format!("binding change tracker socket {}", socket.display()))
}

async fn handle_client(stream: UnixStream, state: Arc<TrackerState>) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) if request.command == "barrier" => {
            match state
                .barrier(&request.outputs, request.since.as_ref())
                .await
            {
                Ok(snapshot) => Response::success(snapshot),
                Err(error) => Response::error(error.to_string()),
            }
        }
        Ok(request) => Response::error(format!("unknown tracker command `{}`", request.command)),
        Err(error) => Response::error(format!("invalid tracker request: {error}")),
    };
    write.write_all(&serde_json::to_vec(&response)?).await?;
    write.write_all(b"\n").await?;
    write.shutdown().await?;
    Ok(())
}

impl TrackerState {
    async fn barrier(
        self: &Arc<Self>,
        outputs: &[String],
        since: Option<&ChangePosition>,
    ) -> Result<ChangeSnapshot> {
        self.ensure_fence_watch()?;
        self.refresh_source_watcher()?;
        self.track_outputs(outputs)?;
        let budget = fence_timeout();
        let per_attempt = budget / FENCE_ATTEMPTS;
        // Retry rather than fail on the first miss. The three calls above
        // can each rebuild the watcher, and a freshly started watch stream
        // does not report a write issued in the same instant, so the first
        // fence after a rebuild can be dropped through no fault of the
        // caller. A live stream answers in milliseconds, so a retry costs
        // nothing on the normal path and is the only thing that rescues the
        // rebuild window.
        for attempt in 1..=FENCE_ATTEMPTS {
            if self.wait_for_fence(per_attempt).await? {
                return Ok(self.snapshot_since(since));
            }
            tracing::debug!(
                attempt,
                attempts = FENCE_ATTEMPTS,
                ?per_attempt,
                "filesystem event barrier missed its fence, retrying"
            );
        }
        anyhow::bail!(
            "timed out after {budget:?} waiting for the filesystem event barrier; \
             set {FENCE_TIMEOUT_ENV} to raise the budget"
        )
    }

    /// Write one fence sentinel and wait up to `timeout` to be told about
    /// it. `Ok(false)` means the wait elapsed, which is retryable.
    async fn wait_for_fence(self: &Arc<Self>, timeout: Duration) -> Result<bool> {
        let token = Uuid::now_v7().to_string();
        let path = self
            .workspace
            .join(".once")
            .join("watch-fences")
            .join(&token);
        let (send, receive) = oneshot::channel();
        self.fences
            .lock()
            .expect("change tracker fence lock poisoned")
            .insert(token.clone(), send);
        if let Err(error) = tokio::fs::write(&path, []).await {
            self.remove_fence(&token);
            return Err(error.into());
        }
        let observed = tokio::time::timeout(timeout, receive).await;
        let _ = tokio::fs::remove_file(path).await;
        if observed.is_err() {
            self.remove_fence(&token);
            return Ok(false);
        }
        Ok(true)
    }

    fn handle_event(&self, event: notify::Result<Event>) {
        let Ok(event) = event else {
            self.record_source_changes(None);
            self.record_output_changes(None);
            return;
        };
        if event.paths.is_empty() {
            self.record_source_changes(None);
            self.record_output_changes(None);
            return;
        }
        let mut source_changes = BTreeSet::new();
        let mut output_changes = BTreeSet::new();
        let mut fence_tokens = Vec::new();
        for path in event.paths {
            let relative = path.strip_prefix(&self.workspace).unwrap_or(&path);
            match fence_class(relative) {
                FenceClass::File(token) => {
                    fence_tokens.push(token);
                    continue;
                }
                FenceClass::Directory => continue,
                FenceClass::NotFence => {}
            }
            match tracked_area(relative) {
                TrackedArea::Source => {
                    tracing::trace!(
                        path = %relative.display(),
                        kind = ?event.kind,
                        "filesystem change tracker observed source change"
                    );
                    source_changes.insert(relative.to_string_lossy().into_owned());
                }
                TrackedArea::Output => {
                    output_changes.insert(relative.to_string_lossy().into_owned());
                }
                TrackedArea::Ignored => {}
            }
        }
        if !source_changes.is_empty() {
            self.record_source_changes(Some(source_changes));
        }
        if !output_changes.is_empty() {
            self.record_output_changes(Some(output_changes));
        }
        // Release every fence barrier only after the changes coalesced into
        // this event have been recorded. Firing earlier would let a waiter
        // snapshot the generation before a same-event change was journaled and
        // wrongly conclude nothing changed.
        for token in fence_tokens {
            self.release_fence(&token);
        }
    }

    fn snapshot_since(&self, since: Option<&ChangePosition>) -> ChangeSnapshot {
        let journal = self
            .journal
            .lock()
            .expect("change tracker journal lock poisoned");
        let position = ChangePosition {
            instance_id: self.instance_id.clone(),
            source_generation: self.source_generation.load(Ordering::Acquire),
            output_generation: self.output_generation.load(Ordering::Acquire),
        };
        let same_instance = since.filter(|since| since.instance_id == self.instance_id);
        let source_changes = same_instance.and_then(|since| {
            changes_since(
                &journal.source,
                since.source_generation,
                position.source_generation,
            )
        });
        let output_changes = same_instance.and_then(|since| {
            changes_since(
                &journal.output,
                since.output_generation,
                position.output_generation,
            )
        });
        ChangeSnapshot {
            position,
            source_changes,
            output_changes,
        }
    }

    fn record_source_changes(&self, paths: Option<BTreeSet<String>>) {
        let mut journal = self
            .journal
            .lock()
            .expect("change tracker journal lock poisoned");
        let generation = self.source_generation.fetch_add(1, Ordering::AcqRel) + 1;
        push_record(&mut journal.source, generation, paths);
    }

    fn record_output_changes(&self, paths: Option<BTreeSet<String>>) {
        let mut journal = self
            .journal
            .lock()
            .expect("change tracker journal lock poisoned");
        let generation = self.output_generation.fetch_add(1, Ordering::AcqRel) + 1;
        push_record(&mut journal.output, generation, paths);
    }

    fn release_fence(&self, token: &str) {
        if let Some(send) = self
            .fences
            .lock()
            .expect("change tracker fence lock poisoned")
            .remove(token)
        {
            let _ = send.send(());
        }
    }

    fn refresh_source_watcher(self: &Arc<Self>) -> Result<()> {
        let current = root_fingerprint(&self.workspace)?;
        let mut prior = self
            .root_fingerprint
            .lock()
            .expect("change tracker root fingerprint lock poisoned");
        if *prior == current {
            return Ok(());
        }
        let callback_state = Arc::clone(self);
        let mut watcher = notify::recommended_watcher(move |event| {
            callback_state.handle_event(event);
        })
        .context("refreshing filesystem change tracker")?;
        watch_sources(&mut watcher, &self.workspace)?;
        for output in self
            .watched_outputs
            .lock()
            .expect("change tracker output set lock poisoned")
            .keys()
        {
            watch_output(&mut watcher, output)?;
        }
        *self
            .source_watcher
            .lock()
            .expect("change tracker watcher lock poisoned") = Some(watcher);
        *prior = current;
        self.record_source_changes(None);
        Ok(())
    }

    fn ensure_fence_watch(&self) -> Result<()> {
        let directory = self.workspace.join(".once").join("watch-fences");
        if directory.is_dir() {
            return Ok(());
        }
        std::fs::create_dir_all(&directory).context("recreating change tracker fence directory")?;
        let mut watched_outputs = self
            .watched_outputs
            .lock()
            .expect("change tracker output set lock poisoned");
        let mut watcher = self
            .source_watcher
            .lock()
            .expect("change tracker watcher lock poisoned");
        let watcher = watcher
            .as_mut()
            .context("filesystem change tracker is not initialized")?;
        for output in watched_outputs.keys() {
            let _ = watcher.unwatch(output);
        }
        watched_outputs.clear();
        watcher
            .watch(&directory, RecursiveMode::Recursive)
            .context("restoring change tracker fence watch")?;
        self.record_source_changes(None);
        Ok(())
    }

    fn track_outputs(&self, outputs: &[String]) -> Result<()> {
        let mut watched_outputs = self
            .watched_outputs
            .lock()
            .expect("change tracker output set lock poisoned");
        let mut watcher = self
            .source_watcher
            .lock()
            .expect("change tracker watcher lock poisoned");
        let watcher = watcher
            .as_mut()
            .context("build output change tracker is not initialized")?;
        for output in outputs {
            let path = self.workspace.join(output);
            if !path.starts_with(self.workspace.join(".once").join("out")) || !path.exists() {
                continue;
            }
            // On Linux an inotify watch is bound to the inode, so replacing a
            // watched output by rename reports the first replacement and then
            // leaves the watch on the dead inode. Without re-pointing it, a
            // later replacement of the same output would go unobserved and an
            // unchanged receipt could certify a stale output.
            //
            // Re-register only when the identity actually moved. Doing it
            // unconditionally churned the watch on every barrier, and because
            // macOS FSEvents is driven by one stream over the whole watch
            // list, every change to that list tears the stream down and
            // rebuilds it. The fence write that immediately follows then raced
            // the restart and its event was dropped, so the barrier waited out
            // its full timeout and callers silently lost the incremental path.
            let identity = OutputIdentity::of(&path);
            if watched_outputs.get(&path) == Some(&identity) {
                continue;
            }
            if watched_outputs.contains_key(&path) {
                let _ = watcher.unwatch(&path);
            }
            watch_output(watcher, &path)?;
            watched_outputs.insert(path, identity);
        }
        Ok(())
    }

    fn remove_fence(&self, token: &str) {
        self.fences
            .lock()
            .expect("change tracker fence lock poisoned")
            .remove(token);
    }
}

fn push_record(
    records: &mut VecDeque<ChangeRecord>,
    generation: u64,
    paths: Option<BTreeSet<String>>,
) {
    records.push_back(ChangeRecord { generation, paths });
    if records.len() > MAX_JOURNAL_RECORDS {
        records.pop_front();
    }
}

fn changes_since(
    records: &VecDeque<ChangeRecord>,
    since_generation: u64,
    current_generation: u64,
) -> Option<Vec<String>> {
    if since_generation > current_generation {
        return None;
    }
    if since_generation == current_generation {
        return Some(Vec::new());
    }
    let mut expected_generation = since_generation + 1;
    let mut changed = BTreeSet::new();
    for record in records
        .iter()
        .filter(|record| record.generation > since_generation)
    {
        if record.generation != expected_generation {
            return None;
        }
        changed.extend(record.paths.as_ref()?.iter().cloned());
        expected_generation += 1;
    }
    (expected_generation == current_generation + 1).then(|| changed.into_iter().collect())
}

/// Filesystem identity of a watched output, used to tell "same file" from
/// "replaced by a rename" without re-registering the watch every time.
///
/// `None` for a path that cannot be stat'd, which compares unequal to a
/// known identity so a vanished output is always re-registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputIdentity(Option<(u64, u64)>);

impl OutputIdentity {
    fn of(path: &Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self(
            std::fs::metadata(path)
                .ok()
                .map(|metadata| (metadata.dev(), metadata.ino())),
        )
    }
}

fn watch_output(watcher: &mut RecommendedWatcher, path: &Path) -> Result<()> {
    let mode = if path.is_dir() {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    watcher
        .watch(path, mode)
        .with_context(|| format!("watching build output {}", path.display()))
}

fn watch_sources(watcher: &mut RecommendedWatcher, workspace: &Path) -> Result<()> {
    let fence_directory = workspace.join(".once").join("watch-fences");
    std::fs::create_dir_all(&fence_directory).context("creating change tracker fence directory")?;
    watcher
        .watch(&fence_directory, RecursiveMode::Recursive)
        .context("watching change tracker fence directory")?;
    for entry in std::fs::read_dir(workspace).context("reading workspace root")? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".once" || name == ".git" {
            continue;
        }
        let mode = if entry.file_type()?.is_dir() {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(&entry.path(), mode)
            .with_context(|| format!("watching source directory {}", entry.path().display()))?;
    }
    Ok(())
}

fn root_fingerprint(workspace: &Path) -> Result<Digest> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(workspace).context("reading workspace root")? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".once" || name == ".git" {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        entries.push((name, metadata.len(), modified));
    }
    entries.sort_unstable();
    let mut bytes = Vec::new();
    for (name, length, modified) in entries {
        let name = name.to_string_lossy();
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(&modified.to_le_bytes());
    }
    Ok(Digest::of_bytes(&bytes))
}

enum TrackedArea {
    Source,
    Output,
    Ignored,
}

/// Classify a path against the fence directory without touching the journal or
/// waking any waiter. `File` carries the fence token; `Directory` is the fence
/// directory itself; `NotFence` is any other path.
#[derive(Debug)]
enum FenceClass {
    NotFence,
    Directory,
    File(String),
}

fn fence_class(relative: &Path) -> FenceClass {
    let mut components = relative.components();
    let is_fence = components
        .next()
        .is_some_and(|part| part.as_os_str() == ".once")
        && components
            .next()
            .is_some_and(|part| part.as_os_str() == "watch-fences");
    if !is_fence {
        return FenceClass::NotFence;
    }
    match components.next().and_then(|part| part.as_os_str().to_str()) {
        Some(token) => FenceClass::File(token.to_owned()),
        None => FenceClass::Directory,
    }
}

fn tracked_area(relative: &Path) -> TrackedArea {
    let mut components = relative.components();
    match components.next().map(std::path::Component::as_os_str) {
        Some(first) if first == ".git" => TrackedArea::Ignored,
        Some(first) if first == ".once" => components.next().map_or(TrackedArea::Ignored, |part| {
            if part.as_os_str() == "out" {
                TrackedArea::Output
            } else {
                TrackedArea::Ignored
            }
        }),
        _ => TrackedArea::Source,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::commands::change_tracker::client::request_snapshot;

    /// Take a barrier snapshot, retrying a transient fence miss and
    /// failing with the underlying reason rather than a bare `unwrap` on
    /// a `None`.
    ///
    /// A single fence can be dropped when the watcher was just rebuilt or
    /// the host is loaded, which says nothing about the behaviour under
    /// test. A cause that persists across every attempt is reported.
    async fn barrier_snapshot(
        socket: &Path,
        outputs: &[String],
        since: Option<&ChangePosition>,
    ) -> ChangeSnapshot {
        let mut last = None;
        for _ in 0..10 {
            match request_snapshot(socket, outputs, since).await {
                Ok(snapshot) => return snapshot,
                Err(error) => {
                    tracing::debug!(%error, "barrier snapshot retry");
                    last = Some(error);
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        match last {
            Some(error) => panic!("barrier snapshot failed: {error}"),
            None => panic!("barrier snapshot never ran"),
        }
    }

    /// Wait until the tracker is accepting connections, rather than
    /// merely until its socket file appears. `bind` publishes the path
    /// before the accept loop runs, so probing for existence let the
    /// test race ahead of the server. A bare connect is the cheap
    /// signal; a barrier round trip here would cost a fence wait per
    /// attempt.
    async fn wait_until_listening(socket: &Path) {
        for _ in 0..400 {
            if tokio::net::UnixStream::connect(socket).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("change tracker never began serving on {}", socket.display());
    }

    /// Barrier repeatedly against a fixed `since` until `ready` accepts
    /// the snapshot.
    ///
    /// A barrier proves the watcher has caught up with its own fence, but
    /// the fence lives in `.once/watch-fences` while sources live
    /// elsewhere, and macOS `FSEvents` coalesces per directory with
    /// independent latency. So observing the fence does not prove a write
    /// to a *different* directory has been delivered yet. Barriers with
    /// the same `since` are idempotent and accumulate, so retrying is the
    /// correct way to assert on an eventually consistent watcher; a fixed
    /// single barrier is a coin flip on macOS.
    async fn barrier_until(
        socket: &Path,
        outputs: &[String],
        since: &ChangePosition,
        what: &str,
        mut ready: impl FnMut(&ChangeSnapshot) -> bool,
    ) -> ChangeSnapshot {
        let mut last = None;
        // Generous because it exits on the first accepting snapshot, so the
        // budget only ever costs anything when the watcher really is slow.
        for _ in 0..200 {
            match request_snapshot(socket, outputs, Some(since)).await {
                Ok(snapshot) => {
                    if ready(&snapshot) {
                        return snapshot;
                    }
                    last = Some(snapshot.position.clone());
                }
                // A transient fence miss is retryable; only a persistent
                // one should fail the test.
                Err(error) => tracing::debug!(%error, "barrier retry"),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("change tracker never observed {what}; last position {last:?}");
    }

    /// Paths reported for an area, treating "unknown" as "none named".
    ///
    /// A `None` change list means the watcher lost track and is telling
    /// callers to assume everything changed. That is a conservative
    /// signal, not a claim that a specific file moved, so a test asking
    /// "was anything in this area actually named?" must read it as empty.
    fn named(changes: Option<&Vec<String>>) -> &[String] {
        changes.map_or(&[], Vec::as_slice)
    }

    /// Barrier until two consecutive snapshots agree on the position,
    /// then return that settled snapshot.
    ///
    /// A newly registered output watch has no stable baseline: the watch
    /// only starts when a barrier first names the output, and macOS
    /// `FSEvents` can then surface a write that predates registration. So
    /// the first barrier over a fresh output may be followed by extra
    /// output generations. For a caller that costs one conservative
    /// revalidation; for a test asserting exact generation stability it
    /// is a coin flip, so settle first and assert from there.
    ///
    /// Settled means a barrier observed no new activity: the position
    /// held still across two consecutive barriers and the second reported
    /// no changes. Checking both together is what makes the quiescent
    /// invariant testable, since asserting it in a separate barrier
    /// afterwards races the next late-arriving coalesced event.
    async fn settled_snapshot(socket: &Path, outputs: &[String]) -> ChangeSnapshot {
        let mut previous = barrier_snapshot(socket, outputs, None).await;
        for _ in 0..50 {
            let next = barrier_snapshot(socket, outputs, Some(&previous.position)).await;
            if next.position == previous.position
                && next.source_changes.as_deref() == Some(&[])
                && next.output_changes.as_deref() == Some(&[])
            {
                return next;
            }
            previous = next;
        }
        panic!("change tracker never settled for outputs {outputs:?}");
    }

    #[tokio::test]
    async fn barriers_distinguish_source_and_final_output_changes() {
        let temporary = TempDir::new().unwrap();
        let workspace = temporary.path().join("workspace");
        let source_directory = workspace.join("src");
        let output = workspace.join(".once/out/app/app");
        tokio::fs::create_dir_all(&source_directory).await.unwrap();
        tokio::fs::create_dir_all(output.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(source_directory.join("main.rs"), b"one")
            .await
            .unwrap();
        tokio::fs::write(&output, b"binary").await.unwrap();
        let socket = temporary.path().join("tracker.sock");
        let server_workspace = workspace.clone();
        let server_socket = socket.clone();
        let task = tokio::spawn(async move {
            serve(&server_workspace, &server_socket).await.unwrap();
        });
        wait_until_listening(&socket).await;

        tokio::fs::write(&output, b"built").await.unwrap();
        // Settling asserts the quiescent invariant: a barrier that sees no
        // filesystem activity holds the position still and reports no
        // source or output changes.
        let initial = settled_snapshot(&socket, &[".once/out/app/app".to_string()]).await;

        tokio::fs::write(source_directory.join("main.rs"), b"two")
            .await
            .unwrap();
        let source_changed = barrier_until(
            &socket,
            &[],
            &initial.position,
            "the source edit",
            |snapshot| snapshot.position.source_generation > initial.position.source_generation,
        )
        .await;
        // `contains` rather than equality: backends coalesce, so an edit
        // to `src/main.rs` legitimately arrives alongside an event for the
        // `src` directory that holds it. Which area a path is classified
        // into is asserted exactly, and without a filesystem, in
        // `classifies_paths_by_tracked_area`.
        assert!(named(source_changed.source_changes.as_ref()).contains(&"src/main.rs".to_string()));

        tokio::fs::write(&output, b"changed").await.unwrap();
        let output_changed = barrier_until(
            &socket,
            &[],
            &source_changed.position,
            "the output write",
            |snapshot| {
                snapshot.position.output_generation > source_changed.position.output_generation
            },
        )
        .await;
        assert!(named(output_changed.output_changes.as_ref())
            .contains(&".once/out/app/app".to_string()));

        tokio::fs::remove_dir_all(workspace.join(".once"))
            .await
            .unwrap();
        let reset = barrier_until(
            &socket,
            &[],
            &output_changed.position,
            "the `.once` removal",
            |snapshot| {
                snapshot.position.source_generation > output_changed.position.source_generation
            },
        )
        .await;
        assert_eq!(reset.source_changes, None);

        tokio::fs::create_dir_all(output.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&output, b"rebuilt").await.unwrap();
        // Removing `.once` dropped the output watch, so this barrier
        // registers it afresh and needs the same settling as the first.
        let rebuilt = settled_snapshot(&socket, &[".once/out/app/app".to_string()]).await;
        tokio::fs::write(&output, b"edited").await.unwrap();
        let rebuilt_output_changed = barrier_until(
            &socket,
            &[],
            &rebuilt.position,
            "the edit to the rebuilt output",
            |snapshot| snapshot.position.output_generation > rebuilt.position.output_generation,
        )
        .await;
        let _ = rebuilt_output_changed;
        task.abort();
    }

    #[tokio::test]
    async fn tracker_exits_after_its_workspace_is_removed() {
        let temporary = TempDir::new().unwrap();
        let workspace = temporary.path().join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let socket = temporary.path().join("tracker.sock");
        let server_workspace = workspace.clone();
        let server_socket = socket.clone();
        let task = tokio::spawn(async move { serve(&server_workspace, &server_socket).await });
        wait_until_listening(&socket).await;

        tokio::fs::remove_dir_all(&workspace).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("tracker did not stop after workspace removal")
            .expect("tracker task panicked")
            .expect("tracker failed while stopping");
        assert!(!socket.exists());
    }

    #[test]
    fn classifies_paths_by_tracked_area() {
        let area = |path: &str| tracked_area(Path::new(path));

        // Anything outside `.once` and `.git` is a source input.
        assert!(matches!(area("src/main.rs"), TrackedArea::Source));
        assert!(matches!(area("Cargo.toml"), TrackedArea::Source));
        assert!(matches!(
            area("deep/nested/file.swift"),
            TrackedArea::Source
        ));
        // A directory named `.once` further down is not the workspace one.
        assert!(matches!(area("vendor/.once/out/x"), TrackedArea::Source));

        // Only `.once/out` holds build outputs, root included.
        assert!(matches!(area(".once/out/app/app"), TrackedArea::Output));
        assert!(matches!(area(".once/out"), TrackedArea::Output));

        // The rest of `.once` is cache and runtime state, and `.git` is
        // never an input.
        assert!(matches!(area(".once/cache/blob"), TrackedArea::Ignored));
        assert!(matches!(area(".once"), TrackedArea::Ignored));
        assert!(matches!(area(".git/HEAD"), TrackedArea::Ignored));
    }

    #[test]
    fn classifies_fence_paths_and_extracts_tokens() {
        let class = |path: &str| fence_class(Path::new(path));

        match class(".once/watch-fences/abc-123") {
            FenceClass::File(token) => assert_eq!(token, "abc-123"),
            other => panic!("expected a fence file, got {other:?}"),
        }
        assert!(matches!(class(".once/watch-fences"), FenceClass::Directory));

        // Fences must never be mistaken for tracked changes, and a
        // similarly named path outside `.once` is not a fence.
        assert!(matches!(class(".once/out/app/app"), FenceClass::NotFence));
        assert!(matches!(class("src/main.rs"), FenceClass::NotFence));
        assert!(matches!(class("watch-fences/abc"), FenceClass::NotFence));
    }

    #[test]
    fn change_journal_reconciles_complete_ranges_and_rejects_gaps() {
        let mut records = VecDeque::new();
        push_record(
            &mut records,
            3,
            Some(BTreeSet::from(["src/a.rs".to_string()])),
        );
        push_record(
            &mut records,
            4,
            Some(BTreeSet::from([
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
            ])),
        );
        assert_eq!(
            changes_since(&records, 2, 4),
            Some(vec!["src/a.rs".to_string(), "src/b.rs".to_string()])
        );
        assert_eq!(changes_since(&records, 4, 4), Some(Vec::new()));
        assert_eq!(changes_since(&records, 1, 4), None);

        push_record(&mut records, 5, None);
        assert_eq!(changes_since(&records, 4, 5), None);
    }
}
