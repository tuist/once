use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use crate::{Error, RemoteExecution, Result};

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
mod input;
#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
mod output;
#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
use super::{join_path, PreparedCommand};
#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
use crate::stream::{self, Destination};
#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
use std::collections::BTreeMap;
#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
use std::time::Duration;

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
mod path {
    use crate::WorkspacePath;

    pub(super) fn guest_path(root: &str, path: &WorkspacePath) -> String {
        super::super::join_path(root, path.as_str())
    }

    pub(super) fn guest_child(parent: &str, name: &str) -> String {
        super::super::join_path(parent, name)
    }
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
pub(super) async fn execute_command(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    let (program, rest) = command.argv.split_first().ok_or(Error::EmptyArgv)?;
    let image = remote.environment.clone().unwrap_or_else(|| {
        std::env::var("ONCE_MICROSANDBOX_IMAGE").unwrap_or_else(|_| "alpine".to_string())
    });
    let guest_root =
        std::env::var("ONCE_MICROSANDBOX_WORKDIR").unwrap_or_else(|_| "/workspace".to_string());
    let guest_cwd = command.cwd.map_or_else(
        || guest_root.clone(),
        |cwd| join_path(&guest_root, cwd.as_str()),
    );
    let sandbox_name = sandbox_name();
    let cpu_count =
        u8::try_from(command.resources.cpu_slots.clamp(1, usize::from(u8::MAX))).unwrap_or(u8::MAX);
    let mut builder = microsandbox::Sandbox::builder(&sandbox_name)
        .image(image)
        .workdir("/")
        .cpus(cpu_count);
    if command.resources.memory_bytes > 0 {
        let memory_mib = u32::try_from(
            command
                .resources
                .memory_bytes
                .div_ceil(1024 * 1024)
                .clamp(1, u64::from(u32::MAX)),
        )
        .unwrap_or(u32::MAX);
        builder = builder.memory(memory_mib);
    }
    let sandbox = builder
        .create()
        .await
        .map_err(|source| microsandbox_error(&source))?;
    let cleanup = SandboxCleanup::new(sandbox.clone());

    let result = async {
        input::stage_inputs(&sandbox.fs(), workspace_root, &guest_root, command.inputs).await?;
        sandbox
            .fs()
            .mkdir(&guest_cwd)
            .await
            .map_err(|source| microsandbox_error(&source))?;

        let work = async {
            let mut handle = sandbox
                .exec_stream_with(program, |exec| {
                    let exec = exec.args(rest.iter().cloned()).cwd(&guest_cwd);
                    command
                        .env
                        .iter()
                        .fold(exec, |exec, (key, value)| exec.env(key, value))
                })
                .await
                .map_err(|source| microsandbox_error(&source))?;
            collect_output(&mut handle, cache, stream_to_parent).await
        };

        let result = match command.timeout_ms {
            Some(ms) => {
                let dur = Duration::from_millis(ms);
                match tokio::time::timeout(dur, work).await {
                    Ok(result) => result,
                    Err(_) => Err(Error::Timeout(dur)),
                }
            }
            None => work.await,
        };

        match result {
            Ok(result) if result.exit_code == 0 => output::retrieve_outputs(
                &sandbox.fs(),
                workspace_root,
                &guest_root,
                command.outputs,
            )
            .await
            .map(|()| result),
            result => result,
        }
    }
    .await;

    let cleanup = cleanup.run().await;

    match (result, cleanup) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => {
            tracing::warn!(provider = "microsandbox", %cleanup_error, "failed to remove remote sandbox after execution failure");
            Err(error)
        }
    }
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
struct SandboxCleanup {
    sandbox: Option<microsandbox::Sandbox>,
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
impl SandboxCleanup {
    fn new(sandbox: microsandbox::Sandbox) -> Self {
        Self {
            sandbox: Some(sandbox),
        }
    }

    async fn run(mut self) -> Result<()> {
        let sandbox = self.sandbox.take().expect("cleanup sandbox is present");
        spawn_cleanup(sandbox)
            .await
            .map_err(|source| Error::RemoteProviderApi {
                provider: "microsandbox".to_string(),
                message: format!("cleanup task failed: {source}"),
            })?
    }
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
impl Drop for SandboxCleanup {
    fn drop(&mut self) {
        let Some(sandbox) = self.sandbox.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(cleanup_sandbox(sandbox));
            }
            Err(error) => {
                tracing::warn!(
                    sandbox = sandbox.name(),
                    %error,
                    "skipping async microsandbox cleanup because no Tokio runtime is available"
                );
            }
        }
    }
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
fn spawn_cleanup(sandbox: microsandbox::Sandbox) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(cleanup_sandbox(sandbox))
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
async fn cleanup_sandbox(sandbox: microsandbox::Sandbox) -> Result<()> {
    let stop_result = sandbox
        .stop_and_wait()
        .await
        .map_err(|source| microsandbox_error(&source));
    let remove_result = sandbox
        .remove_persisted()
        .await
        .map_err(|source| microsandbox_error(&source));
    match (stop_result, remove_result) {
        (Ok(_status), Ok(())) => Ok(()),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[allow(clippy::unused_async)]
#[cfg(not(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
)))]
pub(super) async fn execute_command(
    _remote: &RemoteExecution,
    _command: super::PreparedCommand<'_>,
    _workspace_root: &Path,
    _cache: &CacheProvider,
    _stream_to_parent: bool,
) -> Result<ActionResult> {
    Err(Error::RemoteProviderConfig {
        provider: "microsandbox".to_string(),
        message: microsandbox_unavailable_message().to_string(),
    })
}

#[cfg(not(feature = "remote-microsandbox"))]
fn microsandbox_unavailable_message() -> &'static str {
    "the embedded Microsandbox provider is not enabled in this build"
}

#[cfg(all(
    feature = "remote-microsandbox",
    not(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))
))]
fn microsandbox_unavailable_message() -> &'static str {
    "the embedded Microsandbox provider is only available on Linux and Apple Silicon macOS"
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
async fn collect_output(
    handle: &mut microsandbox::ExecHandle,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    let (mut stdout_reader, mut stdout_writer) = tokio::io::duplex(stream::PIPE_CAPACITY);
    let (mut stderr_reader, mut stderr_writer) = tokio::io::duplex(stream::PIPE_CAPACITY);
    let collect = async {
        let mut exit_code = None;
        while let Some(event) = handle.recv().await {
            match event {
                microsandbox::ExecEvent::Started { .. } => {}
                microsandbox::ExecEvent::Stdout(data) => {
                    stream::write_parent(&data, Destination::Stdout, stream_to_parent).await?;
                    stream::write_pipe(&mut stdout_writer, &data).await?;
                }
                microsandbox::ExecEvent::Stderr(data) => {
                    stream::write_parent(&data, Destination::Stderr, stream_to_parent).await?;
                    stream::write_pipe(&mut stderr_writer, &data).await?;
                }
                microsandbox::ExecEvent::Exited { code } => {
                    exit_code = Some(code);
                    break;
                }
                microsandbox::ExecEvent::Failed(payload) => {
                    return Err(Error::RemoteProviderApi {
                        provider: "microsandbox".to_string(),
                        message: format!("{payload:?}"),
                    });
                }
                microsandbox::ExecEvent::StdinError(payload) => {
                    return Err(Error::RemoteProviderApi {
                        provider: "microsandbox".to_string(),
                        message: format!("{payload:?}"),
                    });
                }
            }
        }
        stream::shutdown_pipe(&mut stdout_writer).await?;
        stream::shutdown_pipe(&mut stderr_writer).await?;
        // A stream that ends without an `Exited` event leaves the real exit
        // status unknown. Surface that explicitly instead of inventing a -1,
        // which would silently report a possibly successful command as failed
        // and skip retrieving its declared outputs.
        exit_code.ok_or_else(|| Error::RemoteProviderApi {
            provider: "microsandbox".to_string(),
            message: "command stream ended without an exit status".to_string(),
        })
    };
    let stdout_store = async {
        cache
            .put_stream(&mut stdout_reader)
            .await
            .map_err(Error::from)
    };
    let stderr_store = async {
        cache
            .put_stream(&mut stderr_reader)
            .await
            .map_err(Error::from)
    };
    let (exit_code, stdout, stderr) = tokio::try_join!(collect, stdout_store, stderr_store)?;
    Ok(ActionResult {
        exit_code,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: BTreeMap::new(),
    })
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
fn microsandbox_error(source: &microsandbox::MicrosandboxError) -> Error {
    Error::RemoteProviderApi {
        provider: "microsandbox".to_string(),
        message: source.to_string(),
    }
}

#[cfg(all(
    feature = "remote-microsandbox",
    any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))
))]
fn sandbox_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("once-{}-{nanos}", std::process::id())
}
