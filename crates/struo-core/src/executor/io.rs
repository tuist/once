use std::time::Duration;

use crate::resources::DEFAULT_ACTION_MEMORY_BYTES;

/// Runtime limits for protocol buffering and artifact staging.
/// These limits do not change action identity or impose guest resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteIoLimits {
    pub max_response_bytes: u64,
    pub max_archive_bytes: u64,
    /// Expanded portable archive bytes per declared download byte.
    pub max_archive_expansion_ratio: u64,
    pub max_staging_bytes: u64,
    /// Command output one hosted action may capture. The whole allowance is
    /// reserved before the sandbox is created, so capture never waits.
    pub max_capture_bytes: u64,
    /// Capture capacity shared by concurrent hosted actions, separate from
    /// transfer staging so neither can starve the other.
    pub max_capture_staging_bytes: u64,
    pub max_archive_entries: usize,
    pub max_metadata_bytes: u64,
    pub stall_timeout: Duration,
    /// Reply wait after an upload body ends, separate from chunk progress.
    pub upload_ack_timeout: Duration,
    /// Absolute download deadline for portable artifact fetches before extraction.
    pub artifact_download_timeout: Duration,
    pub command_idle_timeout: Option<Duration>,
}

impl Default for RemoteIoLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: DEFAULT_ACTION_MEMORY_BYTES / 16,
            max_archive_bytes: 4 * 1024 * 1024 * 1024,
            max_archive_expansion_ratio: 64,
            max_staging_bytes: 8 * 1024 * 1024 * 1024,
            max_capture_bytes: 1024 * 1024 * 1024,
            max_capture_staging_bytes: 8 * 1024 * 1024 * 1024,
            max_archive_entries: 1_000_000,
            max_metadata_bytes: DEFAULT_ACTION_MEMORY_BYTES / 4,
            stall_timeout: Duration::from_secs(30),
            upload_ack_timeout: Duration::from_mins(5),
            artifact_download_timeout: Duration::from_mins(30),
            command_idle_timeout: None,
        }
    }
}

impl RemoteIoLimits {
    pub(crate) fn for_memory(self, memory_bytes: u64) -> Self {
        let memory = if memory_bytes == 0 {
            DEFAULT_ACTION_MEMORY_BYTES
        } else {
            memory_bytes
        };
        Self {
            max_response_bytes: self.max_response_bytes.min(memory / 16),
            max_metadata_bytes: self.max_metadata_bytes.min(memory / 4),
            ..self
        }
    }

    pub(crate) fn control_bytes(self) -> u64 {
        self.max_response_bytes.min(64 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_reservation_bounds_protocol_and_metadata() {
        let limits = RemoteIoLimits::default().for_memory(16384);
        assert_eq!(limits.max_response_bytes, 1024);
        assert_eq!(limits.max_metadata_bytes, 4096);
        assert_eq!(
            limits.max_archive_bytes,
            RemoteIoLimits::default().max_archive_bytes
        );
    }

    #[test]
    fn explicit_tighter_limits_are_preserved() {
        let limits = RemoteIoLimits {
            max_response_bytes: 12,
            max_archive_entries: 1,
            ..RemoteIoLimits::default()
        }
        .for_memory(DEFAULT_ACTION_MEMORY_BYTES);
        assert_eq!(limits.max_response_bytes, 12);
        assert_eq!(limits.max_archive_entries, 1);
    }
}
