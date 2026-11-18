use std::fmt;
use std::io;

use crate::Error;

pub(super) const MAX_WINDOW_LOG: u32 = 23;

#[derive(Debug)]
struct WindowLimitExceeded;

impl fmt::Display for WindowLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote compressed frame exceeds decoder history budget")
    }
}

impl std::error::Error for WindowLimitExceeded {}

pub(super) fn decode_error(error: io::Error) -> io::Error {
    // The zstd wrapper exposes the stable library error name, not its numeric code.
    if error.to_string() == "Frame requires too much memory for decoding" {
        io::Error::new(io::ErrorKind::Unsupported, WindowLimitExceeded)
    } else {
        error
    }
}

pub(super) fn exceeded(error: &Error) -> bool {
    matches!(error, Error::Io { source, .. } if source.get_ref().is_some_and(<dyn std::error::Error + Send + Sync>::is::<WindowLimitExceeded>))
}
