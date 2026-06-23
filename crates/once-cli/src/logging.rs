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
    file_filter_from_env(
        env::var("ONCE_LOG").ok().as_deref(),
        env::var("RUST_LOG").ok().as_deref(),
    )
}

fn stderr_filter(verbose: u8) -> EnvFilter {
    let default = stderr_default_filter(verbose);
    stderr_filter_from_env(env::var("RUST_LOG").ok().as_deref(), default)
}

fn stderr_default_filter(verbose: u8) -> &'static str {
    match verbose {
        0 => "once=warn,once_cli=warn,once_core=warn,once_cas=warn,once_frontend=warn,error",
        1 => "once=info,once_cli=info,once_core=info,once_cas=info,once_frontend=info,error",
        2 => "once=debug,once_cli=debug,once_core=debug,once_cas=debug,once_frontend=debug,error",
        _ => "once=trace,once_cli=trace,once_core=trace,once_cas=trace,once_frontend=trace,error",
    }
}

fn file_filter_from_env(once_log: Option<&str>, rust_log: Option<&str>) -> EnvFilter {
    once_log
        .and_then(|filter| EnvFilter::try_new(filter).ok())
        .or_else(|| rust_log.and_then(|filter| EnvFilter::try_new(filter).ok()))
        .unwrap_or_else(|| EnvFilter::new(INTERNAL_DEFAULT_FILTER))
}

fn stderr_filter_from_env(rust_log: Option<&str>, default: &str) -> EnvFilter {
    rust_log
        .and_then(|filter| EnvFilter::try_new(filter).ok())
        .unwrap_or_else(|| EnvFilter::new(default))
}

fn log_dir() -> std::io::Result<PathBuf> {
    log_dir_from(platform_log_dir())
}

fn log_dir_from(platform_dir: Option<PathBuf>) -> std::io::Result<PathBuf> {
    platform_dir.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no platform log directory")
    })
}

#[cfg(target_os = "macos")]
fn platform_log_dir() -> Option<PathBuf> {
    platform_log_dir_from(home_dir())
}

#[cfg(target_os = "macos")]
fn platform_log_dir_from(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|home| home.join("Library").join("Logs").join("Once"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_log_dir() -> Option<PathBuf> {
    platform_log_dir_from(xdg_state_home(), home_dir())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_log_dir_from(xdg_state: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg_state
        .or_else(|| home.map(|home| home.join(".local").join("state")))
        .map(|state| state.join("once").join("logs"))
}

#[cfg(windows)]
fn platform_log_dir() -> Option<PathBuf> {
    platform_log_dir_from(env::var_os("LOCALAPPDATA").map(PathBuf::from))
}

#[cfg(windows)]
fn platform_log_dir_from(local_app_data: Option<PathBuf>) -> Option<PathBuf> {
    local_app_data.map(|local| local.join("Once").join("Logs"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_state_home() -> Option<PathBuf> {
    xdg_state_home_from(env::var_os("XDG_STATE_HOME").map(PathBuf::from))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_state_home_from(raw: Option<PathBuf>) -> Option<PathBuf> {
    raw.filter(|path| path.is_absolute())
}

#[cfg(unix)]
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::filter::LevelFilter;

    #[test]
    fn file_filter_prefers_once_log_over_rust_log() {
        let filter = file_filter_from_env(Some("once=trace"), Some("once=warn"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::TRACE));
    }

    #[test]
    fn file_filter_falls_back_to_rust_log() {
        let filter = file_filter_from_env(None, Some("once=info"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::INFO));
    }

    #[test]
    fn file_filter_uses_internal_default() {
        let filter = file_filter_from_env(None, None);
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::DEBUG));
    }

    #[test]
    fn stderr_filter_prefers_rust_log_over_verbose_default() {
        let filter = stderr_filter_from_env(Some("once=trace"), "warn");
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::TRACE));
    }

    #[test]
    fn stderr_filter_uses_verbose_default() {
        let filter = stderr_filter_from_env(None, "info");
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::INFO));
    }

    #[test]
    fn stderr_default_filter_scopes_dependency_warnings() {
        assert_eq!(
            stderr_default_filter(0),
            "once=warn,once_cli=warn,once_core=warn,once_cas=warn,once_frontend=warn,error"
        );
    }

    #[test]
    fn log_dir_errors_without_platform_directory() {
        let error = log_dir_from(None).expect_err("missing log dir should error");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn log_dir_returns_platform_directory() {
        let expected = PathBuf::from("/tmp/once-logs");
        assert_eq!(log_dir_from(Some(expected.clone())).unwrap(), expected);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn platform_log_dir_uses_macos_library_logs() {
        assert_eq!(
            platform_log_dir_from(Some(PathBuf::from("/Users/test"))).unwrap(),
            PathBuf::from("/Users/test/Library/Logs/Once")
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn platform_log_dir_uses_xdg_state_home() {
        assert_eq!(
            platform_log_dir_from(
                Some(PathBuf::from("/state")),
                Some(PathBuf::from("/home/test"))
            )
            .unwrap(),
            PathBuf::from("/state/once/logs")
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn platform_log_dir_falls_back_to_home_state() {
        assert_eq!(
            platform_log_dir_from(None, Some(PathBuf::from("/home/test"))).unwrap(),
            PathBuf::from("/home/test/.local/state/once/logs")
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn xdg_state_home_requires_absolute_path() {
        assert_eq!(
            xdg_state_home_from(Some(PathBuf::from("relative/state"))),
            None
        );
        assert_eq!(
            xdg_state_home_from(Some(PathBuf::from("/absolute/state"))).unwrap(),
            PathBuf::from("/absolute/state")
        );
    }

    #[test]
    #[cfg(windows)]
    fn platform_log_dir_uses_local_app_data() {
        assert_eq!(
            platform_log_dir_from(Some(PathBuf::from(r"C:\Users\test\AppData\Local"))).unwrap(),
            PathBuf::from(r"C:\Users\test\AppData\Local\Once\Logs")
        );
    }
}
