use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use once_cas::{ActionResult, CacheProvider};
use tokio::process::{Child, Command};

use crate::executor::staging::CapturePermit;
use crate::stream::{Destination, PIPE_CAPACITY};
use crate::{Error, RemoteIoLimits, Result};

mod connection;
mod io;
use io::{bounded_bytes, pump};
mod transfer;

#[cfg(all(test, unix))]
mod tests;

#[derive(Clone)]
pub(super) struct Client {
    pub binary: std::ffi::OsString,
    pub limits: RemoteIoLimits,
    connection: std::sync::Arc<connection::Connection>,
}

pub(super) fn error(message: impl Into<String>) -> Error {
    Error::RemoteProviderApi {
        provider: "docker".into(),
        message: message.into(),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn io_error(source: std::io::Error) -> Error {
    error(format!("Docker process I/O failed: {source}"))
}

impl Client {
    pub async fn verify_output_tar(&self, container: &str) -> Result<()> {
        let help = self
            .control(&[
                "exec".into(),
                container.into(),
                "tar".into(),
                "--help".into(),
            ])
            .await?;
        if !String::from_utf8_lossy(&help)
            .split_ascii_whitespace()
            .any(|word| word == "--hard-dereference")
        {
            return Err(error(
                "Docker output collection requires tar with --hard-dereference support",
            ));
        }
        Ok(())
    }

    pub async fn verify_image(&self, image: &str) -> Result<()> {
        let inspected = self
            .control(&[
                "image".into(),
                "inspect".into(),
                "--format".into(),
                "{{.Id}}".into(),
                image.into(),
            ])
            .await?;
        if std::str::from_utf8(&inspected).map_or(true, |id| id.trim() != image) {
            return Err(error(
                "Docker image inspection did not match the bound immutable image ID",
            ));
        }
        Ok(())
    }

    fn spawn(&self, args: &[String], input: bool) -> Result<Child> {
        let mut command = Command::new(&self.binary);
        self.connection.configure(&mut command);
        command
            .args(args)
            .stdin(if input { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(io_error)
    }

    pub async fn control(&self, args: &[String]) -> Result<Vec<u8>> {
        let mut child = self.spawn(args, false)?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let limit = self.limits.control_bytes();
        let work = async {
            let (status, stdout, _) = tokio::try_join!(
                async { child.wait().await.map_err(io_error) },
                bounded_bytes(stdout, limit),
                bounded_bytes(stderr, limit),
            )?;
            tracing::debug!(
                executor = "docker",
                operation = args.first().map(String::as_str),
                exit_code = status.code(),
                "Docker control command completed"
            );
            if !status.success() {
                return Err(error(format!(
                    "Docker control command failed with {status}"
                )));
            }
            Ok(stdout)
        };
        tokio::time::timeout(Duration::from_mins(1), work)
            .await
            .map_err(|_| Error::Timeout(Duration::from_mins(1)))?
    }

    pub async fn execute(
        &self,
        args: &[String],
        cache: &CacheProvider,
        parent: bool,
        budget: Option<CapturePermit>,
    ) -> Result<ActionResult> {
        let mut child = self.spawn(args, false)?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (out_reader, mut out_writer) = tokio::io::duplex(PIPE_CAPACITY);
        let (err_reader, mut err_writer) = tokio::io::duplex(PIPE_CAPACITY);
        let total = AtomicU64::new(0);
        let (status, (), (), stdout, stderr) = tokio::try_join!(
            async { child.wait().await.map_err(io_error) },
            pump(
                stdout,
                &mut out_writer,
                self.limits.max_capture_bytes,
                Some((&total, budget.as_ref())),
                Some((Destination::Stdout, parent))
            ),
            pump(
                stderr,
                &mut err_writer,
                self.limits.max_capture_bytes,
                Some((&total, budget.as_ref())),
                Some((Destination::Stderr, parent))
            ),
            async { cache.put_stream(out_reader).await.map_err(Error::from) },
            async { cache.put_stream(err_reader).await.map_err(Error::from) },
        )?;
        let exit_code = status
            .code()
            .ok_or_else(|| error("Docker exec ended without an exit code"))?;
        tracing::debug!(
            executor = "docker",
            exit_code,
            captured_bytes = total.load(std::sync::atomic::Ordering::Relaxed),
            "Docker action command completed"
        );
        Ok(ActionResult {
            exit_code,
            stdout: Some(stdout),
            stderr: Some(stderr),
            outputs: BTreeMap::new(),
        })
    }
}
