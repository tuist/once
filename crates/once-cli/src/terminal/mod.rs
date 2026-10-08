mod console;
mod frames;
mod policy;
mod status;
mod text;

pub(crate) use console::{output, ConsoleOut, ConsoleWriter, PanelGuard};
pub(crate) use frames::SyncFrames;
pub(crate) use policy::Policy;
pub(crate) use status::{install_panic_hook, StatusGuard};
pub(crate) use text::captured_text;

use std::io::{self, Write};

pub(crate) async fn report_cancellation(report: impl FnOnce() + Send + 'static) {
    let (finished, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        status::cancel_active();
        report();
        let _ = finished.send(());
    });
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), receiver).await;
}

#[derive(Debug)]
pub(crate) struct StderrWriter;

impl Write for StderrWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::stderr().lock().write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().lock().flush()
    }
}
