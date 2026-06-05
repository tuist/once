use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{Action, CacheState, OutputSymlinkMode, ResourceRequest, RunOpts, WorkspacePath};

/// Result type used by the high-level Once SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors returned by the high-level Once SDK.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("argv cannot be empty")]
    EmptyArgv,
    #[error("invalid workspace path: {0}")]
    InvalidWorkspacePath(#[from] once_core::WorkspacePathError),
    #[error(transparent)]
    Run(#[from] once_core::Error),
}

/// Embeddable Once runtime bound to one workspace and one cache.
///
/// `Once` is cheap to clone and can be reused across many command runs.
/// The default cache provider is local filesystem storage rooted at
/// `cache_root`, usually the workspace's `.once` directory.
#[derive(Clone)]
pub struct Once {
    workspace_root: PathBuf,
    cache: CacheProvider,
    opts: RunOpts,
    stream: bool,
}

impl Once {
    /// Create a runtime that stores cache data in `cache_root`.
    ///
    /// `workspace_root` is the absolute or process-relative root that
    /// workspace-relative command paths resolve against.
    pub fn new(workspace_root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self::with_cache(
            workspace_root,
            CacheProvider::open_local(cache_root),
            RunOpts::default(),
        )
    }

    /// Create a runtime with an explicit cache provider.
    pub fn with_cache(
        workspace_root: impl Into<PathBuf>,
        cache: CacheProvider,
        opts: RunOpts,
    ) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            cache,
            opts,
            stream: false,
        }
    }

    /// Return a copy that streams subprocess output to this process.
    #[must_use]
    pub fn streaming(mut self, enabled: bool) -> Self {
        self.stream = enabled;
        self
    }

    /// Return a copy that caches non-zero command exits.
    ///
    /// This is disabled by default because infra failures should not
    /// normally become durable cache entries.
    #[must_use]
    pub fn cache_failures(mut self, enabled: bool) -> Self {
        self.opts.cache_failures = enabled;
        self
    }

    /// Workspace root used by this runtime.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Cache provider used by this runtime.
    pub fn cache(&self) -> &CacheProvider {
        &self.cache
    }

    /// Run a command through Once's cache-aware executor.
    pub async fn run_command(&self, command: Command) -> Result<CommandOutcome> {
        let action = command.into_action()?;
        let outcome = if self.stream {
            once_core::run_with_cache_streaming(
                &action,
                &self.workspace_root,
                &self.cache,
                self.opts,
            )
            .await?
        } else {
            once_core::run_with_cache(&action, &self.workspace_root, &self.cache, self.opts).await?
        };
        Ok(CommandOutcome::from_core(outcome))
    }
}

/// A command that can be executed by Once.
///
/// Paths such as `cwd` and `outputs` are workspace-relative and are
/// validated before execution. `input_digest` is optional; integrations
/// that know their source inputs should pass one so cache keys change
/// when those inputs change.
#[derive(Debug, Clone)]
pub struct Command {
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    input_digest: Option<Digest>,
    outputs: Vec<String>,
    timeout_ms: Option<u64>,
}

impl Command {
    /// Start a command with the executable name or path.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            argv: vec![program.into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: Vec::new(),
            timeout_ms: None,
        }
    }

    /// Start a command from a full argv vector.
    pub fn from_argv(argv: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: Vec::new(),
            timeout_ms: None,
        }
    }

    /// Append one argument.
    #[must_use]
    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.argv.push(value.into());
        self
    }

    /// Append many arguments.
    #[must_use]
    pub fn args(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.argv.extend(values.into_iter().map(Into::into));
        self
    }

    /// Set one environment variable for the command.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set the workspace-relative current directory.
    #[must_use]
    pub fn cwd(mut self, path: impl Into<String>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Attach a caller-computed input digest.
    #[must_use]
    pub fn input_digest(mut self, digest: Digest) -> Self {
        self.input_digest = Some(digest);
        self
    }

    /// Declare a workspace-relative output path to restore on hits.
    #[must_use]
    pub fn output(mut self, path: impl Into<String>) -> Self {
        self.outputs.push(path.into());
        self
    }

    /// Set a command timeout in milliseconds.
    #[must_use]
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub(crate) fn into_action(self) -> Result<Action> {
        if self.argv.is_empty() {
            return Err(Error::EmptyArgv);
        }
        let cwd = self
            .cwd
            .as_deref()
            .map(WorkspacePath::try_from)
            .transpose()?;
        let outputs = self
            .outputs
            .iter()
            .map(|path| WorkspacePath::try_from(path.as_str()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Action::RunCommand {
            argv: self.argv,
            env: self.env,
            cwd,
            input_digest: self.input_digest,
            outputs,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            timeout_ms: self.timeout_ms,
            remote: None,
        })
    }
}

/// Result of a cache-aware command run.
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    /// Digest of the action key used for lookup and storage.
    pub action_digest: Digest,
    /// Whether the result came from cache or fresh execution.
    pub cache: CacheState,
    /// Process exit code.
    pub exit_code: i32,
    /// Full cached action result, including stdout, stderr, and output
    /// digests.
    pub result: ActionResult,
}

impl CommandOutcome {
    fn from_core(outcome: once_core::Outcome) -> Self {
        Self {
            action_digest: outcome.action,
            cache: outcome.cache,
            exit_code: outcome.result.exit_code,
            result: outcome.result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_builder_validates_workspace_paths() {
        let error = Command::new("true")
            .output("../outside")
            .into_action()
            .unwrap_err();
        assert!(matches!(error, Error::InvalidWorkspacePath(_)));
    }

    #[tokio::test]
    async fn run_command_hits_cache_after_first_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let once = Once::new(tmp.path(), tmp.path().join(".once"));
        let command = || Command::new("sh").arg("-c").arg("printf hello");

        let first = once.run_command(command()).await.unwrap();
        let second = once.run_command(command()).await.unwrap();

        assert_eq!(first.exit_code, 0);
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(first.action_digest, second.action_digest);
    }
}
