use std::fs::{self, File, OpenOptions, TryLockError};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const DIRECTORY: &str = ".struo/executors/lifecycle";
const QUARANTINE: &str = "quarantine";
const RECORD_EXTENSION: &str = "json";
const LOCK_EXTENSION: &str = "lock";
const TEMPORARY_EXTENSION: &str = "tmp";
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const MAX_QUARANTINED: usize = 16;
const CAPACITY_LOCK: &str = "capacity.lock";
/// Records this workspace may hold at once. A full journal fails new creates
/// closed rather than letting untracked sandboxes accumulate.
pub(in crate::remote) const MAX_RECORDS: usize = 256;
/// Directory entries examined by capacity checks and scans: every record,
/// its lock, and room for temporary files and the quarantine directory.
pub(in crate::remote) const MAX_ENTRIES: usize = MAX_RECORDS * 2 + 64;
const MAX_SANDBOX_IDS: usize = 8;
const STALE_TEMPORARY: Duration = Duration::from_mins(10);
const VERSION: u32 = 1;
const NONCE_LENGTH: usize = 32;

/// Workspace-local record of hosted sandboxes this workspace may own. Each
/// record is one file guarded by a sibling lock file: a live owner keeps the
/// lock for the record's whole lifetime, so a reconciler that cannot take the
/// lock knows the operation is still in progress.
#[derive(Clone, Debug)]
pub(in crate::remote) struct Journal {
    root: PathBuf,
    /// Whether writes are flushed to stable storage. Production journals
    /// always sync; unit tests opt in with [`Journal::durable`] because the
    /// macOS flush is device-wide and dozens of concurrently journaling tests
    /// would otherwise queue behind each other and trip unrelated timing
    /// assertions.
    synced: bool,
    /// Counts flushes so tests can prove which writes were synced.
    #[cfg(test)]
    syncs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::remote) enum Phase {
    /// A create request may have been sent; the provider id is unknown.
    Intent,
    /// The provider returned at least one sandbox id that must be deleted.
    Created,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::remote) struct Record {
    pub version: u32,
    pub provider: String,
    pub scope: String,
    /// Fingerprint of the credential that created the record.
    pub credential: String,
    pub nonce: String,
    pub phase: Phase,
    #[serde(default)]
    pub sandbox_ids: Vec<String>,
    pub created_at: u64,
    /// Earliest time at which an authenticated, complete provider listing
    /// without the marker is accepted as proof that the create never landed.
    pub settles_at: u64,
    pub attempts: u32,
    pub next_check_at: u64,
}

/// An exclusively locked journal record.
#[derive(Debug)]
pub(in crate::remote) struct Entry {
    journal: Journal,
    record: Record,
    _lock: File,
}

pub(in crate::remote) enum Claim {
    Entry(Box<Entry>),
    Active,
    Missing,
    Quarantined,
}

impl Journal {
    pub(in crate::remote) fn new(workspace_root: &Path) -> Self {
        Self {
            root: workspace_root.join(DIRECTORY),
            synced: !cfg!(test),
            #[cfg(test)]
            syncs: std::sync::Arc::default(),
        }
    }

    /// A journal that flushes like production, for durability tests.
    #[cfg(test)]
    pub(in crate::remote) fn durable(workspace_root: &Path) -> Self {
        Self {
            synced: true,
            ..Self::new(workspace_root)
        }
    }

    #[cfg(test)]
    pub(in crate::remote) fn root(&self) -> &Path {
        &self.root
    }

    /// Flushes performed through this journal and its clones.
    #[cfg(test)]
    pub(in crate::remote) fn syncs(&self) -> usize {
        self.syncs.load(Ordering::SeqCst)
    }

    fn sync_file(&self, file: &File) -> io::Result<()> {
        if !self.synced {
            return Ok(());
        }
        #[cfg(test)]
        self.syncs.fetch_add(1, Ordering::SeqCst);
        file.sync_all()
    }

    /// Makes renames and removals in the journal directory durable. Not every
    /// platform can open a directory for syncing, so this is best effort.
    fn sync_directory(&self, directory: &Path) {
        if !self.synced || !cfg!(unix) {
            return;
        }
        #[cfg(test)]
        self.syncs.fetch_add(1, Ordering::SeqCst);
        if let Ok(directory) = File::open(directory) {
            let _ = directory.sync_all();
        }
    }

    /// Records the intent to create a sandbox before any request is sent.
    /// Fails when the journal is full, so no paid create runs untracked.
    pub(in crate::remote) fn begin(
        &self,
        provider: &str,
        scope: &str,
        credential: &str,
        settle: Duration,
    ) -> Result<Entry> {
        fs::create_dir_all(&self.root).map_err(|source| self.error("create", source))?;
        let capacity = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(CAPACITY_LOCK))
            .map_err(|source| self.error("lock", source))?;
        capacity
            .lock()
            .map_err(|source| self.error("lock", source))?;
        self.ensure_capacity(provider)?;
        let nonce = nonce();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(self.path(&nonce, LOCK_EXTENSION))
            .map_err(|source| self.error("lock", source))?;
        lock.try_lock()
            .map_err(|error| self.error("lock", lock_error(error)))?;
        let now = now();
        let record = Record {
            version: VERSION,
            provider: provider.to_string(),
            scope: scope.to_string(),
            credential: credential.to_string(),
            nonce,
            phase: Phase::Intent,
            sandbox_ids: Vec::new(),
            created_at: now,
            settles_at: now.saturating_add(settle.as_secs()),
            attempts: 0,
            next_check_at: now,
        };
        let entry = Entry {
            journal: self.clone(),
            record,
            _lock: lock,
        };
        if let Err(error) = entry.persist(true) {
            let _ = fs::remove_file(self.path(&entry.record.nonce, LOCK_EXTENSION));
            return Err(error);
        }
        drop(capacity);
        Ok(entry)
    }

    fn ensure_capacity(&self, provider: &str) -> Result<()> {
        let mut entries = 0;
        let mut records = 0;
        for entry in fs::read_dir(&self.root)
            .map_err(|source| self.error("scan", source))?
            .take(MAX_ENTRIES + 1)
        {
            entries += 1;
            let is_record = entry.ok().is_some_and(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| parse_name(name, RECORD_EXTENSION).is_some())
            });
            records += usize::from(is_record);
        }
        if records >= MAX_RECORDS || entries > MAX_ENTRIES {
            tracing::warn!(
                provider,
                records,
                entries,
                "remote lifecycle journal is full"
            );
            return Err(Error::ExecutorResourceLimit {
                executor: provider.to_string(),
                resource: "remote lifecycle journal records",
                limit: MAX_RECORDS as u64,
            });
        }
        Ok(())
    }

    /// Lists record nonces, least recently written first so records deferred
    /// by a pass rotate behind those not yet examined. Stale temporary files
    /// and orphaned locks are removed along the way. At most `limit`
    /// directory entries are examined; [`Journal::begin`] keeps the journal
    /// within [`MAX_ENTRIES`], so a default scan sees every record.
    pub(in crate::remote) fn scan(&self, limit: usize) -> Result<Vec<String>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(self.error("scan", source)),
        };
        let mut nonces = Vec::new();
        let mut locks = Vec::new();
        for entry in entries.take(limit) {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.ends_with(TEMPORARY_EXTENSION) {
                remove_if_stale(&path);
            } else if let Some(nonce) = parse_name(name, RECORD_EXTENSION) {
                let written = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(UNIX_EPOCH);
                nonces.push((written, nonce));
            } else if let Some(nonce) = parse_name(name, LOCK_EXTENSION) {
                locks.push(nonce);
            }
        }
        for nonce in locks {
            if !nonces.iter().any(|(_, record)| *record == nonce) {
                self.remove_orphan_lock(&nonce);
            }
        }
        nonces.sort();
        Ok(nonces.into_iter().map(|(_, nonce)| nonce).collect())
    }

    /// Takes ownership of a record unless a live owner holds it.
    pub(in crate::remote) fn claim(&self, nonce: &str) -> Result<Claim> {
        let lock_path = self.path(nonce, LOCK_EXTENSION);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| self.error("lock", source))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(Claim::Active),
            Err(TryLockError::Error(source)) => return Err(self.error("lock", source)),
        }
        let record_path = self.path(nonce, RECORD_EXTENSION);
        let bytes = match read_bounded(&record_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let _ = fs::remove_file(&lock_path);
                return Ok(Claim::Missing);
            }
            Err(source) => return Err(self.error("read", source)),
        };
        match serde_json::from_slice::<Record>(&bytes) {
            Ok(record) if record.is_valid(nonce) => Ok(Claim::Entry(Box::new(Entry {
                journal: self.clone(),
                record,
                _lock: lock,
            }))),
            _ => {
                self.quarantine(nonce)?;
                let _ = fs::remove_file(&lock_path);
                Ok(Claim::Quarantined)
            }
        }
    }

    /// Moves an unreadable record aside so it is never acted on, keeping at
    /// most a fixed number of quarantined files for diagnosis.
    fn quarantine(&self, nonce: &str) -> Result<()> {
        let directory = self.root.join(QUARANTINE);
        fs::create_dir_all(&directory).map_err(|source| self.error("quarantine", source))?;
        let target = directory.join(format!("{nonce}.{}.{RECORD_EXTENSION}", now_nanos()));
        fs::rename(self.path(nonce, RECORD_EXTENSION), target)
            .map_err(|source| self.error("quarantine", source))?;
        self.sync_directory(&self.root);
        tracing::warn!(nonce, "quarantined unreadable remote lifecycle record");
        let mut quarantined = fs::read_dir(&directory)
            .map_err(|source| self.error("quarantine", source))?
            .take(MAX_QUARANTINED * 4)
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect::<Vec<_>>();
        quarantined.sort();
        let excess = quarantined.len().saturating_sub(MAX_QUARANTINED);
        for (_, path) in quarantined.into_iter().take(excess) {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }

    fn remove_orphan_lock(&self, nonce: &str) {
        let path = self.path(nonce, LOCK_EXTENSION);
        let Ok(lock) = OpenOptions::new().read(true).write(true).open(&path) else {
            return;
        };
        if lock.try_lock().is_ok() && !self.path(nonce, RECORD_EXTENSION).exists() {
            let stale = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > STALE_TEMPORARY));
            if stale {
                let _ = fs::remove_file(&path);
            }
        }
    }

    fn path(&self, nonce: &str, extension: &str) -> PathBuf {
        self.root.join(format!("{nonce}.{extension}"))
    }

    fn error(&self, operation: &'static str, source: io::Error) -> Error {
        Error::FileAction {
            action: match operation {
                "create" => "create_remote_lifecycle_journal",
                "lock" => "lock_remote_lifecycle_record",
                "scan" => "scan_remote_lifecycle_journal",
                "read" => "read_remote_lifecycle_record",
                "quarantine" => "quarantine_remote_lifecycle_record",
                "remove" => "remove_remote_lifecycle_record",
                _ => "write_remote_lifecycle_record",
            },
            path: self.root.display().to_string(),
            source,
        }
    }
}

impl Record {
    fn is_valid(&self, nonce: &str) -> bool {
        self.version == VERSION
            && self.nonce == nonce
            && !self.provider.is_empty()
            && !self.scope.is_empty()
            && !self.credential.is_empty()
            && self.sandbox_ids.len() <= MAX_SANDBOX_IDS
            && (self.phase == Phase::Intent || !self.sandbox_ids.is_empty())
    }
}

impl Entry {
    pub(in crate::remote) fn record(&self) -> &Record {
        &self.record
    }

    pub(in crate::remote) fn nonce(&self) -> &str {
        &self.record.nonce
    }

    /// Persists the provider ids before the caller uses the sandbox.
    pub(in crate::remote) fn mark_created(&mut self, ids: &[String]) -> Result<()> {
        let mut ids = ids.to_vec();
        ids.truncate(MAX_SANDBOX_IDS);
        self.record.phase = Phase::Created;
        self.record.sandbox_ids = ids;
        self.persist(false)
    }

    /// Keeps the record for a later reconciliation pass.
    pub(in crate::remote) fn defer(&mut self, retry_after: Duration) -> Result<()> {
        self.record.attempts = self.record.attempts.saturating_add(1);
        self.record.next_check_at = now().saturating_add(retry_after.as_secs());
        self.persist(false)
    }

    /// Removes the record once the sandbox is known to be gone. The removal
    /// is not synced: a record that reappears after a crash is verified by
    /// marker again and resolved, never acted on blindly.
    pub(in crate::remote) fn complete(self) -> Result<()> {
        let record = self.journal.path(&self.record.nonce, RECORD_EXTENSION);
        match fs::remove_file(&record) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(self.journal.error("remove", source)),
        }
        let _ = fs::remove_file(self.journal.path(&self.record.nonce, LOCK_EXTENSION));
        Ok(())
    }

    /// Durability is scoped to what reconciliation needs. The intent is the
    /// one write that must survive a crash: its contents and the directory
    /// entry are flushed before the create request is sent, so an operation
    /// that may have created a sandbox always leaves a marker to look up.
    /// Later updates flush their contents but not the directory, and removal
    /// is not flushed at all. A crash can therefore roll a record back to an
    /// earlier valid version or resurrect a removed one; both are harmless
    /// because every reconciliation deletes only what a fresh, authenticated
    /// lookup of the marker returns. Contents are always flushed before the
    /// rename, so a record is never observed torn.
    fn persist(&self, new: bool) -> Result<()> {
        let bytes = serde_json::to_vec(&self.record)
            .map_err(|source| self.journal.error("write", io::Error::other(source)))?;
        self.journal
            .write_atomic(
                &format!("{}.{RECORD_EXTENSION}", self.record.nonce),
                &bytes,
                new,
            )
            .map_err(|source| self.journal.error("write", source))
    }
}

impl Journal {
    /// Writes through a synced temporary file and renames it into place,
    /// optionally syncing the directory so the rename itself survives a crash.
    fn write_atomic(&self, name: &str, bytes: &[u8], sync_directory: bool) -> io::Result<()> {
        let temporary = self
            .root
            .join(format!("{name}.{}.{TEMPORARY_EXTENSION}", now_nanos()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            self.sync_file(&file)?;
            drop(file);
            fs::rename(&temporary, self.root.join(name))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        if sync_directory {
            self.sync_directory(&self.root);
        }
        Ok(())
    }
}

/// Oversized records read as empty so they are quarantined, not parsed.
fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        bytes.clear();
    }
    Ok(bytes)
}

fn remove_if_stale(path: &Path) {
    let stale = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > STALE_TEMPORARY));
    if stale {
        let _ = fs::remove_file(path);
    }
}

fn parse_name(name: &str, extension: &str) -> Option<String> {
    let nonce = name.strip_suffix(extension)?.strip_suffix('.')?;
    is_nonce(nonce).then(|| nonce.to_string())
}

pub(in crate::remote) fn is_nonce(value: &str) -> bool {
    value.len() == NONCE_LENGTH
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn lock_error(error: TryLockError) -> io::Error {
    match error {
        TryLockError::WouldBlock => io::Error::new(io::ErrorKind::WouldBlock, "record is locked"),
        TryLockError::Error(error) => error,
    }
}

/// A unique operation marker. Uniqueness, not secrecy, is what matters, so
/// randomly keyed `SipHash` outputs plus process and clock state are enough.
fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = blake3::Hasher::new();
    for _ in 0..2 {
        let mut random = std::collections::hash_map::RandomState::new().build_hasher();
        random.write_u64(counter);
        hasher.update(&random.finish().to_le_bytes());
    }
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&now_nanos().to_le_bytes());
    hasher.update(&counter.to_le_bytes());
    hasher.finalize().to_hex()[..NONCE_LENGTH].to_string()
}

pub(in crate::remote) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}
