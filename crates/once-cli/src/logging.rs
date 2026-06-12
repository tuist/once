use std::env;
use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use uuid::Uuid;

const INTERNAL_DEFAULT_FILTER: &str =
    "once=debug,once_cli=debug,once_core=debug,once_cas=debug,once_frontend=debug,warn";

pub struct Logging {
    session_id: Uuid,
    log_path: Option<PathBuf>,
    _guard: Option<WorkerGuard>,
}

impl Logging {
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }
}

pub fn init(verbose: u8) -> Logging {
    let session_id = Uuid::now_v7();
    let stderr_filter = stderr_filter(verbose);

    if let Ok((dir, (writer, guard))) =
        log_dir().and_then(|dir| file_writer(&dir, session_id).map(|writer| (dir, writer)))
    {
        let log_path = dir.join(format!("{session_id}.log"));
        let file_filter = file_filter();
        let file_layer = fmt::layer()
            .json()
            .with_ansi(false)
            .with_current_span(true)
            .with_span_list(true)
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_writer(writer)
            .with_filter(file_filter);
        let stderr_layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(stderr_filter);

        tracing_subscriber::registry()
            .with(file_layer)
            .with(stderr_layer)
            .init();

        return Logging {
            session_id,
            log_path: Some(log_path),
            _guard: Some(guard),
        };
    }

    fmt()
        .with_env_filter(stderr_filter)
        .with_writer(std::io::stderr)
        .init();

    Logging {
        session_id,
        log_path: None,
        _guard: None,
    }
}

fn file_writer(
    dir: &Path,
    session_id: Uuid,
) -> std::io::Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    std::fs::create_dir_all(dir)?;
    let appender = tracing_appender::rolling::never(dir, format!("{session_id}.log"));
    Ok(tracing_appender::non_blocking(appender))
}

fn file_filter() -> EnvFilter {
    EnvFilter::try_from_env("ONCE_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(INTERNAL_DEFAULT_FILTER))
}

fn stderr_filter(verbose: u8) -> EnvFilter {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
}

fn log_dir() -> std::io::Result<PathBuf> {
    platform_log_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no platform log directory")
    })
}

#[cfg(target_os = "macos")]
fn platform_log_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Library").join("Logs").join("Once"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_log_dir() -> Option<PathBuf> {
    xdg_state_home()
        .or_else(|| home_dir().map(|home| home.join(".local").join("state")))
        .map(|state| state.join("once").join("logs"))
}

#[cfg(windows)]
fn platform_log_dir() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|local| local.join("Once").join("Logs"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_state_home() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(unix)]
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
