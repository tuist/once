//! Bounded retention for per-session log files.
//!
//! Every invocation writes one `<uuidv7>.log`. Without a bound the directory
//! grows forever, so each session prunes the oldest logs once it has created
//! its own. Version 7 UUID names sort by creation time, so ordering needs only the
//! directory listing. Files that are not session logs are never touched.

use std::path::Path;

use uuid::Uuid;

/// Session logs kept by default, counting the current one.
pub(super) const DEFAULT_MAX_LOGS: usize = 100;
/// Total size of the kept session logs by default.
pub(super) const DEFAULT_MAX_BYTES: u64 = 500 * 1024 * 1024;
/// The newest logs kept even when they exceed the size bound, so one large
/// session cannot erase the history around it.
const MIN_LOGS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Retention {
    pub max_logs: usize,
    pub max_bytes: u64,
}

impl Retention {
    /// Defaults, overridden by `ONCE_LOG_RETAINED_SESSIONS` and
    /// `ONCE_LOG_RETAINED_BYTES`. Unparsable values keep the default.
    pub(super) fn from_env() -> Self {
        Self::from_values(
            std::env::var("ONCE_LOG_RETAINED_SESSIONS").ok().as_deref(),
            std::env::var("ONCE_LOG_RETAINED_BYTES").ok().as_deref(),
        )
    }

    fn from_values(sessions: Option<&str>, bytes: Option<&str>) -> Self {
        Self {
            max_logs: sessions
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(DEFAULT_MAX_LOGS),
            max_bytes: bytes
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(DEFAULT_MAX_BYTES),
        }
    }
}

/// Most logs one session deletes. Pruning runs on the startup path, so a large
/// backlog from before retention existed drains over several sessions instead
/// of stalling one.
const MAX_REMOVALS_PER_SESSION: usize = 1000;

/// Keep the newest session logs in `dir` that fit `retention`, always including
/// `current`, and delete older ones. Only kept logs and removed logs are
/// stat'd, so the steady state costs one directory listing. Returns how many
/// were removed. Failures are ignored: retention is housekeeping and must not
/// stop a command.
pub(super) fn prune(dir: &Path, current: Uuid, retention: Retention) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut sessions = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let id = Uuid::parse_str(name.to_str()?.strip_suffix(".log")?).ok()?;
            Some((id, entry.path()))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.0));

    let mut kept = 0usize;
    let mut kept_bytes = 0u64;
    let mut keeping = true;
    let mut removed = 0;
    for (id, path) in sessions {
        if !keeping && id != current {
            if removed >= MAX_REMOVALS_PER_SESSION {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let len = metadata.len();
        if id != current {
            let within_size =
                kept < MIN_LOGS || kept_bytes.saturating_add(len) <= retention.max_bytes;
            keeping = kept < retention.max_logs && within_size;
            if !keeping {
                if std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
                continue;
            }
        }
        kept += 1;
        kept_bytes = kept_bytes.saturating_add(len);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(dir: &Path, bytes: usize) -> Uuid {
        let id = Uuid::now_v7();
        std::fs::write(dir.join(format!("{id}.log")), vec![b'x'; bytes]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        id
    }

    fn remaining(dir: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn keeps_the_newest_logs_up_to_the_count() {
        let tmp = tempfile::tempdir().unwrap();
        let ids = (0..6).map(|_| session(tmp.path(), 1)).collect::<Vec<_>>();
        let retention = Retention {
            max_logs: 4,
            max_bytes: u64::MAX,
        };

        assert_eq!(prune(tmp.path(), ids[5], retention), 2);
        let expected = ids[2..]
            .iter()
            .map(|id| format!("{id}.log"))
            .collect::<Vec<_>>();
        assert_eq!(remaining(tmp.path()), expected);
    }

    #[test]
    fn the_size_bound_spares_the_newest_few_and_the_current_session() {
        let tmp = tempfile::tempdir().unwrap();
        let ids = (0..6).map(|_| session(tmp.path(), 100)).collect::<Vec<_>>();
        // Only one log fits in the byte budget, yet the newest three stay.
        let retention = Retention {
            max_logs: 100,
            max_bytes: 150,
        };

        assert_eq!(prune(tmp.path(), ids[5], retention), 3);
        let expected = ids[3..]
            .iter()
            .map(|id| format!("{id}.log"))
            .collect::<Vec<_>>();
        assert_eq!(remaining(tmp.path()), expected);
    }

    #[test]
    fn the_current_session_survives_a_zero_count() {
        let tmp = tempfile::tempdir().unwrap();
        let older = session(tmp.path(), 1);
        let current = session(tmp.path(), 1);

        prune(
            tmp.path(),
            current,
            Retention {
                max_logs: 0,
                max_bytes: 0,
            },
        );
        assert_eq!(remaining(tmp.path()), vec![format!("{current}.log")]);
        assert_ne!(older, current);
    }

    #[test]
    fn a_large_backlog_drains_over_several_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        for _ in 0..MAX_REMOVALS_PER_SESSION + 4 {
            std::fs::write(tmp.path().join(format!("{}.log", Uuid::now_v7())), b"x").unwrap();
        }
        let current = session(tmp.path(), 1);
        let retention = Retention {
            max_logs: 1,
            max_bytes: u64::MAX,
        };

        assert_eq!(
            prune(tmp.path(), current, retention),
            MAX_REMOVALS_PER_SESSION
        );
        assert_eq!(remaining(tmp.path()).len(), 5);
        assert_eq!(prune(tmp.path(), current, retention), 4);
        assert_eq!(remaining(tmp.path()), vec![format!("{current}.log")]);
    }

    #[test]
    fn files_that_are_not_session_logs_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.log"), b"keep").unwrap();
        std::fs::write(tmp.path().join(format!("{}.txt", Uuid::now_v7())), b"keep").unwrap();
        let current = session(tmp.path(), 1);

        prune(
            tmp.path(),
            current,
            Retention {
                max_logs: 1,
                max_bytes: 0,
            },
        );
        assert_eq!(remaining(tmp.path()).len(), 3);
    }

    #[test]
    fn environment_values_override_the_defaults() {
        assert_eq!(
            Retention::from_values(Some("7"), Some("1024")),
            Retention {
                max_logs: 7,
                max_bytes: 1024
            }
        );
        assert_eq!(
            Retention::from_values(Some("many"), None),
            Retention {
                max_logs: DEFAULT_MAX_LOGS,
                max_bytes: DEFAULT_MAX_BYTES
            }
        );
    }
}
