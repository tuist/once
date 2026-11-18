//! Decide whether a path's metadata can vouch for its contents yet.
//!
//! Stat-based caches assume that any write moves a file's size, modification
//! time, or inode change time. Filesystem timestamps advance in clock ticks
//! rather than nanoseconds, though: on many kernels and filesystems two writes
//! a few milliseconds apart get the same timestamps, so a same-length rewrite
//! right after a description was taken leaves every recorded field unchanged.
//! Git calls such entries racily clean.
//!
//! A description is only trusted once everything it covers stopped changing
//! longer ago than any timestamp granularity in use. Until then it carries a
//! marker unique to the moment it was taken, so it equals no other description
//! and callers fall back to reading contents.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a path must go unchanged before its description is trusted, when
/// its timestamps carry sub-second precision. Ten times the coarsest kernel
/// clock tick, which is what such timestamps advance by.
pub const SETTLE_WINDOW: Duration = Duration::from_millis(100);

/// The same for whole-second timestamps, longer than the two seconds FAT-family
/// volumes round modification times to.
const WHOLE_SECOND_SETTLE_WINDOW: Duration = Duration::from_secs(3);

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// `None` when every timestamp on `metadata` is older than the settle window,
/// otherwise a value no other call returns, to mix into the description built
/// from `metadata` so that description cannot match a later one.
#[must_use]
pub fn unsettled_stamp(metadata: &std::fs::Metadata) -> Option<u128> {
    let now = SystemTime::now();
    if let Some(newest) = newest_change(metadata) {
        let since_epoch = newest.duration_since(UNIX_EPOCH).unwrap_or_default();
        let window = if since_epoch.subsec_nanos() == 0 {
            WHOLE_SECOND_SETTLE_WINDOW
        } else {
            SETTLE_WINDOW
        };
        if newest
            .checked_add(window)
            .is_some_and(|settled| settled <= now)
        {
            return None;
        }
    }
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Some(
        (nanos << 64)
            | (u128::from(std::process::id()) << 32)
            | u128::from(COUNTER.fetch_add(1, Ordering::Relaxed)),
    )
}

fn newest_change(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    let modified = metadata.modified().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let changed = UNIX_EPOCH.checked_add(Duration::new(
            u64::try_from(metadata.ctime()).ok()?,
            u32::try_from(metadata.ctime_nsec()).ok()?,
        ))?;
        Some(modified.max(changed))
    }
    #[cfg(not(unix))]
    {
        Some(modified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_writes_are_unsettled_and_never_repeat_a_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        std::fs::write(&path, b"a").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();

        let first = unsettled_stamp(&metadata).expect("a fresh write is unsettled");
        let second = unsettled_stamp(&metadata).expect("a fresh write is unsettled");

        assert_ne!(first, second);
    }

    #[test]
    fn files_left_alone_past_the_window_are_settled() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        std::fs::write(&path, b"a").unwrap();
        std::thread::sleep(SETTLE_WINDOW * 2);

        assert_eq!(unsettled_stamp(&std::fs::metadata(&path).unwrap()), None);
    }

    #[cfg(unix)]
    #[test]
    fn restoring_an_old_modification_time_does_not_settle_a_fresh_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        std::fs::write(&path, b"a").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();

        assert!(unsettled_stamp(&std::fs::metadata(&path).unwrap()).is_some());
    }
}
