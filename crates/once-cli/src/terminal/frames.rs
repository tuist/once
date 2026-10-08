use std::fmt;
use std::io::{self, Write};
use std::sync::Mutex;

use indicatif::TermLike;

use super::console::WriteScope;
use super::StderrWriter;

const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";
const FRAME_CAP: usize = 64 * 1024;

struct FrameWriter<W> {
    writer: W,
    bytes: Vec<u8>,
    passthrough: bool,
}

impl<W: Write> FrameWriter<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            bytes: Vec::new(),
            passthrough: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.passthrough {
            return self.writer.write_all(bytes);
        }
        let prefix = if self.bytes.is_empty() {
            BEGIN.len()
        } else {
            0
        };
        let exceeds_cap = self
            .bytes
            .len()
            .saturating_add(prefix)
            .saturating_add(bytes.len())
            > FRAME_CAP + BEGIN.len();
        if exceeds_cap
            || self
                .bytes
                .try_reserve(prefix + bytes.len() + END.len())
                .is_err()
        {
            self.passthrough = true;
            let buffered = std::mem::take(&mut self.bytes);
            if !buffered.is_empty() {
                self.writer.write_all(&buffered[BEGIN.len()..])?;
            }
            return self.writer.write_all(bytes);
        }
        if self.bytes.is_empty() {
            self.bytes.extend_from_slice(BEGIN);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.passthrough {
            self.passthrough = false;
            return self.writer.flush();
        }
        if self.bytes.is_empty() {
            return Ok(());
        }
        self.bytes.extend_from_slice(END);
        let frame = std::mem::take(&mut self.bytes);
        let result = self
            .writer
            .write_all(&frame)
            .and_then(|()| self.writer.flush());
        if result.is_err() {
            let _ = self.writer.write_all(END);
            let _ = self.writer.flush();
        }
        result
    }
}

pub(crate) struct SyncFrames {
    frame: Mutex<FrameWriter<StderrWriter>>,
    term: console::Term,
}

impl fmt::Debug for SyncFrames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SyncFrames")
    }
}

impl SyncFrames {
    pub(crate) fn stderr() -> Self {
        Self {
            frame: Mutex::new(FrameWriter::new(StderrWriter)),
            term: console::Term::stderr(),
        }
    }

    fn lock_frame(&self) -> std::sync::MutexGuard<'_, FrameWriter<StderrWriter>> {
        self.frame.lock().unwrap_or_else(|e| {
            let mut frame = e.into_inner();
            frame.bytes.clear();
            frame.passthrough = false;
            self.frame.clear_poison();
            frame
        })
    }

    fn push(&self, bytes: &[u8]) -> io::Result<()> {
        let _scope = WriteScope::enter();
        self.lock_frame().push(bytes)
    }

    fn move_cursor(&self, count: usize, direction: char) -> io::Result<()> {
        if count == 0 {
            return Ok(());
        }
        self.push(format!("\x1b[{count}{direction}").as_bytes())
    }
}

impl TermLike for SyncFrames {
    fn width(&self) -> u16 {
        self.term.size().1
    }
    fn height(&self) -> u16 {
        self.term.size().0
    }
    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'A')
    }
    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'B')
    }
    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'C')
    }
    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'D')
    }
    fn clear_line(&self) -> io::Result<()> {
        self.push(b"\r\x1b[2K")
    }
    fn write_str(&self, text: &str) -> io::Result<()> {
        self.push(text.as_bytes())
    }
    fn write_line(&self, text: &str) -> io::Result<()> {
        self.push(text.as_bytes())?;
        self.push(b"\n")
    }
    fn flush(&self) -> io::Result<()> {
        let _scope = WriteScope::enter();
        self.lock_frame().flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        writes: Vec<Vec<u8>>,
        fail: bool,
    }
    impl Write for Recorder {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes.push(bytes.to_vec());
            if std::mem::take(&mut self.fail) {
                Err(io::Error::other("injected failure"))
            } else {
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn frame_is_buffered_and_emitted_with_balanced_markers() {
        let mut frame = FrameWriter::new(Recorder::default());
        frame.flush().unwrap();
        assert!(frame.writer.writes.is_empty());
        frame.push(b"\rredraw").unwrap();
        assert!(frame.writer.writes.is_empty());
        frame.flush().unwrap();
        assert_eq!(
            frame.writer.writes,
            [b"\x1b[?2026h\rredraw\x1b[?2026l".to_vec()]
        );
    }

    #[test]
    fn overflow_keeps_every_byte_without_synchronization() {
        let mut frame = FrameWriter::new(Recorder::default());
        let content = vec![b'x'; FRAME_CAP];
        frame.push(&content).unwrap();
        frame.push(b"overflow").unwrap();
        frame.flush().unwrap();
        assert_eq!(
            frame.writer.writes.concat(),
            [content, b"overflow".to_vec()].concat()
        );
        frame.writer.writes.clear();
        frame.push(b"next").unwrap();
        frame.flush().unwrap();
        assert_eq!(
            frame.writer.writes,
            [b"\x1b[?2026hnext\x1b[?2026l".to_vec()]
        );
    }

    #[test]
    fn failure_attempts_reset_and_does_not_replay_a_frame() {
        let mut frame = FrameWriter::new(Recorder {
            fail: true,
            ..Recorder::default()
        });
        frame.push(b"failed").unwrap();
        assert!(frame.flush().is_err());
        assert_eq!(frame.writer.writes.last().unwrap(), END);
        frame.writer.writes.clear();
        frame.push(b"new").unwrap();
        frame.flush().unwrap();
        assert_eq!(frame.writer.writes, [b"\x1b[?2026hnew\x1b[?2026l".to_vec()]);
    }

    #[test]
    fn at_capacity_still_emits_a_synchronized_frame() {
        let mut frame = FrameWriter::new(Recorder::default());
        frame.push(&vec![b'x'; FRAME_CAP]).unwrap();
        frame.flush().unwrap();
        assert_eq!(frame.writer.writes.len(), 1);
        assert!(frame.writer.writes[0].starts_with(BEGIN));
        assert!(frame.writer.writes[0].ends_with(END));
    }

    #[test]
    fn passthrough_failure_has_no_markers() {
        let mut frame = FrameWriter::new(Recorder {
            fail: true,
            ..Recorder::default()
        });
        assert!(frame.push(&vec![b'x'; FRAME_CAP + 1]).is_err());
        assert!(!frame
            .writer
            .writes
            .concat()
            .windows(BEGIN.len())
            .any(|w| w == BEGIN));
        frame.flush().unwrap();
        frame.push(b"new").unwrap();
        frame.flush().unwrap();
        assert!(frame.writer.writes.last().unwrap().starts_with(BEGIN));
    }

    #[test]
    fn a_poisoned_frame_is_reset_once_without_discarding_later_pushes() {
        let frame = SyncFrames::stderr();
        frame.push(b"stale").unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = frame.frame.lock().unwrap();
            panic!("injected panic");
        }));
        frame.push(b"new").unwrap();
        frame.push(b" content").unwrap();
        assert!(!frame.frame.is_poisoned());
        assert_eq!(frame.frame.lock().unwrap().bytes, b"\x1b[?2026hnew content");
    }

    #[test]
    fn zero_cursor_movement_is_a_noop() {
        let frame = SyncFrames::stderr();
        frame.move_cursor_up(0).unwrap();
        assert!(frame.frame.lock().unwrap().bytes.is_empty());
    }
}
