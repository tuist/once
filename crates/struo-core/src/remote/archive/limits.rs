use std::io::{self, Write};

use crate::{Error, Result};

#[derive(Debug, thiserror::Error)]
#[error("archive exceeds staging limit")]
struct LimitExceeded(u64);

pub(super) struct LimitedWriter<W> {
    inner: W,
    written: u64,
    limit: u64,
}

impl<W> LimitedWriter<W> {
    pub(super) fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }

    pub(super) fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(bytes.len() as u64) > self.limit {
            return Err(io::Error::other(LimitExceeded(self.limit)));
        }
        let written = self.inner.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(super) fn limit_error(provider: &'static str, source: &io::Error) -> Option<Error> {
    let limit = source.get_ref()?.downcast_ref::<LimitExceeded>()?;
    Some(Error::ExecutorResourceLimit {
        executor: provider.to_string(),
        resource: "archive bytes",
        limit: limit.0,
    })
}

pub(super) fn check_entries(count: usize, limit: usize, provider: &'static str) -> Result<()> {
    super::super::http::check_size(count as u64, limit as u64, provider, "archive entries")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_growth_before_writing_it() {
        let mut writer = LimitedWriter::new(Vec::new(), 5);
        writer.write_all(b"abc").unwrap();
        let error = writer.write_all(b"def").unwrap_err();
        assert!(matches!(
            limit_error("test", &error),
            Some(Error::ExecutorResourceLimit { limit: 5, .. })
        ));
        assert_eq!(writer.into_inner(), b"abc");
    }
}
