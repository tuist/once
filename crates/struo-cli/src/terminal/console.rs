use std::cell::Cell;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use indicatif::{MultiProgress, ProgressDrawTarget};
use tracing_subscriber::fmt::MakeWriter;

use super::StderrWriter;

const LINE_LIMIT: usize = 64 * 1024;

thread_local! {
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

pub(super) struct WriteScope(bool);

impl WriteScope {
    pub(super) fn enter() -> Self {
        Self(WRITING.replace(true))
    }
}

impl Drop for WriteScope {
    fn drop(&mut self) {
        WRITING.set(self.0);
    }
}

pub(crate) struct ConsoleOut {
    panel: Mutex<Option<Arc<MultiProgress>>>,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl Default for ConsoleOut {
    fn default() -> Self {
        Self {
            panel: Mutex::new(None),
            writer: Mutex::new(Box::new(StderrWriter)),
        }
    }
}

pub(crate) fn output() -> &'static ConsoleOut {
    static OUTPUT: OnceLock<ConsoleOut> = OnceLock::new();
    OUTPUT.get_or_init(ConsoleOut::default)
}

impl ConsoleOut {
    pub(crate) fn line(&self, text: &str) {
        let _ = self.bytes(text.as_bytes());
    }

    fn bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let _scope = WriteScope::enter();
        let panel = self
            .panel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(panel) = panel.as_deref() {
            let text = if bytes.len() > LINE_LIMIT {
                let prefix = &bytes[..LINE_LIMIT];
                let valid = std::str::from_utf8(prefix).map_or_else(|e| e.valid_up_to(), str::len);
                format!(
                    "{}\n[terminal output truncated; full output remains in the log or cache]",
                    super::captured_text(&prefix[..valid])
                )
            } else {
                String::from_utf8_lossy(bytes).into_owned()
            };
            panel.println(text.strip_suffix('\n').unwrap_or(&text))
        } else {
            self.writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .write_all(bytes)
        }
    }

    pub(crate) fn register(&'static self, multi: Arc<MultiProgress>) -> PanelGuard {
        *self
            .panel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(multi.clone());
        PanelGuard {
            console: self,
            multi,
        }
    }
}

pub(crate) struct PanelGuard {
    console: &'static ConsoleOut,
    multi: Arc<MultiProgress>,
}

impl Drop for PanelGuard {
    fn drop(&mut self) {
        let _scope = WriteScope::enter();
        let mut panel = self
            .console
            .panel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if panel
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &self.multi))
        {
            let _ = self.multi.clear();
            self.multi.set_draw_target(ProgressDrawTarget::hidden());
            *panel = None;
        } else {
            self.multi.set_draw_target(ProgressDrawTarget::hidden());
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ConsoleWriter(&'static ConsoleOut);

impl ConsoleWriter {
    pub(crate) fn new() -> Self {
        Self(output())
    }
}

impl<'a> MakeWriter<'a> for ConsoleWriter {
    type Writer = EventWriter;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            console: self.0,
            suppressed: WRITING.get(),
        }
    }
}

pub(crate) struct EventWriter {
    console: &'static ConsoleOut,
    suppressed: bool,
}

impl Write for EventWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.suppressed {
            self.console.bytes(bytes)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::{ProgressBar, TermLike};

    #[derive(Clone, Default, Debug)]
    struct Recorder(Arc<Mutex<Vec<u8>>>);
    impl Write for Recorder {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl TermLike for Recorder {
        fn width(&self) -> u16 {
            100
        }
        fn move_cursor_up(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_down(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_left(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_right(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn clear_line(&self) -> io::Result<()> {
            Ok(())
        }
        fn write_str(&self, text: &str) -> io::Result<()> {
            self.clone().write_all(text.as_bytes())
        }
        fn write_line(&self, text: &str) -> io::Result<()> {
            self.write_str(&format!("{text}\n"))
        }
        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    fn sink() -> (&'static ConsoleOut, Recorder) {
        let recorder = Recorder::default();
        let sink = Box::leak(Box::new(ConsoleOut {
            panel: Mutex::new(None),
            writer: Mutex::new(Box::new(recorder.clone())),
        }));
        (sink, recorder)
    }

    fn warn(writer: impl for<'a> MakeWriter<'a> + Send + Sync + 'static) {
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, || tracing::warn!("same bytes"));
    }

    #[test]
    fn absent_panel_preserves_tracing_bytes() {
        let expected = Recorder::default();
        let captured = expected.clone();
        warn(move || captured.clone());
        let (sink, actual) = sink();
        warn(ConsoleWriter(sink));
        assert_eq!(*actual.0.lock().unwrap(), *expected.0.lock().unwrap());
    }

    #[test]
    fn tracing_uses_the_panel_and_teardown_stops_late_redraws() {
        let (sink, raw) = sink();
        let drawn = Recorder::default();
        let multi = Arc::new(MultiProgress::with_draw_target(
            ProgressDrawTarget::term_like(Box::new(drawn.clone())),
        ));
        let panel = sink.register(multi.clone());
        let bar = multi.add(ProgressBar::new_spinner());
        bar.set_message("working");
        warn(ConsoleWriter(sink));
        assert!(raw.0.lock().unwrap().is_empty());
        assert!(String::from_utf8_lossy(&drawn.0.lock().unwrap()).contains("same bytes"));
        drop(panel);
        let length = drawn.0.lock().unwrap().len();
        bar.tick();
        drop(bar);
        assert_eq!(drawn.0.lock().unwrap().len(), length);
        warn(ConsoleWriter(sink));
        assert!(String::from_utf8_lossy(&raw.0.lock().unwrap()).contains("same bytes"));
    }

    #[test]
    fn oversized_panel_messages_truncate_without_splitting_utf8() {
        let (sink, _) = sink();
        let drawn = Recorder::default();
        let multi = Arc::new(MultiProgress::with_draw_target(
            ProgressDrawTarget::term_like(Box::new(drawn.clone())),
        ));
        let _panel = sink.register(multi);
        sink.line(&format!(
            "{}é{}",
            "x".repeat(LINE_LIMIT - 1),
            "y".repeat(100)
        ));
        let bytes = drawn.0.lock().unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("terminal output truncated"));
        assert!(!text.contains('\u{fffd}'));
    }

    #[test]
    fn recursive_logs_do_not_reenter_the_console() {
        let (sink, actual) = sink();
        let _scope = WriteScope::enter();
        warn(ConsoleWriter(sink));
        assert!(actual.0.lock().unwrap().is_empty());
    }
}
