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
    watched_outputs: Mutex<BTreeSet<PathBuf>>,
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
        watched_outputs: Mutex::new(BTreeSet::new()),
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
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("accepting tracker client")?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, state).await {
                tracing::debug!(%error, "filesystem change tracker client failed");
            }
        });
    }
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
        let observed = tokio::time::timeout(Duration::from_secs(2), receive).await;
        let _ = tokio::fs::remove_file(path).await;
        if observed.is_err() {
            self.remove_fence(&token);
            anyhow::bail!("timed out waiting for filesystem event barrier");
        }
        Ok(self.snapshot_since(since))
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
            .iter()
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
        for output in &*watched_outputs {
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
            if watched_outputs.contains(&path)
                || !path.starts_with(self.workspace.join(".once").join("out"))
                || !path.exists()
            {
                continue;
            }
            watch_output(watcher, &path)?;
            watched_outputs.insert(path);
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
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        tokio::fs::write(&output, b"built").await.unwrap();
        let initial = request_snapshot(&socket, &[".once/out/app/app".to_string()], None)
            .await
            .unwrap();
        let settled = request_snapshot(&socket, &[], Some(&initial.position))
            .await
            .unwrap();
        assert_eq!(settled.position, initial.position);
        assert_eq!(settled.source_changes, Some(Vec::new()));
        assert_eq!(settled.output_changes, Some(Vec::new()));

        tokio::fs::write(source_directory.join("main.rs"), b"two")
            .await
            .unwrap();
        let source_changed = request_snapshot(&socket, &[], Some(&initial.position))
            .await
            .unwrap();
        assert!(source_changed.position.source_generation > initial.position.source_generation);
        assert_eq!(
            source_changed.position.output_generation,
            initial.position.output_generation
        );
        assert_eq!(
            source_changed.source_changes,
            Some(vec!["src/main.rs".to_string()])
        );

        tokio::fs::write(&output, b"changed").await.unwrap();
        let output_changed = request_snapshot(&socket, &[], Some(&source_changed.position))
            .await
            .unwrap();
        assert_eq!(
            output_changed.position.source_generation,
            source_changed.position.source_generation
        );
        assert!(
            output_changed.position.output_generation > source_changed.position.output_generation
        );
        assert_eq!(
            output_changed.output_changes,
            Some(vec![".once/out/app/app".to_string()])
        );

        tokio::fs::remove_dir_all(workspace.join(".once"))
            .await
            .unwrap();
        let reset = request_snapshot(&socket, &[], Some(&output_changed.position))
            .await
            .unwrap();
        assert!(reset.position.source_generation > output_changed.position.source_generation);
        assert_eq!(reset.source_changes, None);

        tokio::fs::create_dir_all(output.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&output, b"rebuilt").await.unwrap();
        let rebuilt = request_snapshot(
            &socket,
            &[".once/out/app/app".to_string()],
            Some(&reset.position),
        )
        .await
        .unwrap();
        tokio::fs::write(&output, b"edited").await.unwrap();
        let rebuilt_output_changed = request_snapshot(&socket, &[], Some(&rebuilt.position))
            .await
            .unwrap();
        assert!(
            rebuilt_output_changed.position.output_generation > rebuilt.position.output_generation
        );
        task.abort();
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
