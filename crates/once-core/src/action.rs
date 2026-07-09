use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::LazyLock;

use once_cas::Digest;
use serde::{Deserialize, Serialize};

use crate::{ResourceRequest, WorkspacePath};

/// Domain-separation prefix for action digests. Bump the version when
/// the canonical encoding (or the [`Action`] schema) changes in a way
/// that should invalidate the cache. Older action result JSON still
/// deserializes through serde defaults; the domain only partitions new
/// action lookups.
pub(crate) const ACTION_DIGEST_DOMAIN: &[u8] = b"once.action.v7\0";

static DEFAULT_RESOURCE_REQUEST: LazyLock<ResourceRequest> =
    LazyLock::new(ResourceRequest::default);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputSymlinkMode {
    Preserve,
    #[default]
    MaterializeExternal,
}

impl OutputSymlinkMode {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl FromStr for OutputSymlinkMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "preserve" => Ok(Self::Preserve),
            "materialize-external" => Ok(Self::MaterializeExternal),
            _ => Err(format!(
                "expected `preserve` or `materialize-external`, got `{raw}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    #[default]
    Off,
    Inputs,
}

impl SandboxMode {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn stronger(self, other: Self) -> Self {
        std::cmp::max(self, other)
    }
}

impl FromStr for SandboxMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "off" => Ok(Self::Off),
            "inputs" => Ok(Self::Inputs),
            _ => Err(format!("expected `off` or `inputs`, got `{raw}`")),
        }
    }
}

/// All actions Once can execute.
///
/// The wire format of this enum is part of the action digest (see
/// `ACTION_DIGEST_DOMAIN`). Field additions, renames, or reorderings
/// that affect the JSON encoding require a digest version bump.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    RunCommand {
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspacePath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        inputs: Vec<WorkspacePath>,
        /// Workspace-relative paths the action promises to produce. The
        /// runner stores each one in the CAS after a fresh execution
        /// and restores it from the CAS on a cache hit. An empty list
        /// means the action has no declared outputs (only stdout/stderr
        /// are cached); cache hits then provide nothing on disk.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<WorkspacePath>,
        /// Redirect the child's stdout into this workspace-relative file
        /// instead of capturing it into the CAS as a stream. The file is
        /// an ordinary declared output (list it in `outputs` to cache and
        /// restore it). None keeps the default stream capture. Boxed to
        /// keep the common (unset) case from enlarging the enum.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_path: Option<Box<WorkspacePath>>,
        /// Redirect the child's stderr into this workspace-relative file.
        /// When it equals `stdout_path` the two streams share one file
        /// handle, reproducing shell `2>&1`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_path: Option<Box<WorkspacePath>>,
        #[serde(default, skip_serializing_if = "OutputSymlinkMode::is_default")]
        output_symlink_mode: OutputSymlinkMode,
        #[serde(default, skip_serializing_if = "ResourceRequest::is_default")]
        resources: ResourceRequest,
        #[serde(default, skip_serializing_if = "SandboxMode::is_default")]
        sandbox: SandboxMode,
        /// Per-action timeout in milliseconds. None = no timeout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        /// Optional compute provider for remote execution. This is
        /// part of the action key so local and remote runs never share
        /// a cache slot by accident.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<RemoteExecution>,
    },
    WriteFile {
        path: WorkspacePath,
        bytes: Vec<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
    CopyPath {
        sources: Vec<WorkspacePath>,
        destination: WorkspacePath,
        mode: CopyPathMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
    PreparePath {
        path: WorkspacePath,
        mode: PreparePathMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
    WriteTreeDigest {
        root: WorkspacePath,
        output: WorkspacePath,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        include_suffixes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CopyPathMode {
    File,
    Tree,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreparePathMode {
    Remove,
    Directory,
}

impl Action {
    /// Canonical, content-addressed key for this action.
    ///
    /// The key is `BLAKE3(domain || canonical_json(self))`. Bumping the
    /// domain partitions old and new cache entries cleanly instead of
    /// silently colliding.
    pub fn digest(&self) -> Digest {
        let body = serde_json::to_vec(self).expect("Action is serializable");
        let mut buf = Vec::with_capacity(ACTION_DIGEST_DOMAIN.len() + body.len());
        buf.extend_from_slice(ACTION_DIGEST_DOMAIN);
        buf.extend_from_slice(&body);
        Digest::of_bytes(&buf)
    }

    pub fn resource_request(&self) -> &ResourceRequest {
        match self {
            Action::RunCommand { resources, .. } => resources,
            Action::WriteFile { .. }
            | Action::CopyPath { .. }
            | Action::PreparePath { .. }
            | Action::WriteTreeDigest { .. } => &DEFAULT_RESOURCE_REQUEST,
        }
    }

    pub fn input_digest(&self) -> Option<Digest> {
        match self {
            Action::RunCommand { input_digest, .. }
            | Action::WriteFile { input_digest, .. }
            | Action::CopyPath { input_digest, .. }
            | Action::PreparePath { input_digest, .. }
            | Action::WriteTreeDigest { input_digest, .. } => *input_digest,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RemoteExecution {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl RemoteExecution {
    pub fn provider(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            account: None,
            project: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(output_symlink_mode: OutputSymlinkMode) -> Action {
        Action::RunCommand {
            argv: vec!["true".to_string()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![WorkspacePath::try_from("out").unwrap()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode,
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            remote: None,
        }
    }

    #[test]
    fn output_symlink_mode_changes_action_digest() {
        assert_ne!(
            action(OutputSymlinkMode::MaterializeExternal).digest(),
            action(OutputSymlinkMode::Preserve).digest()
        );
    }

    #[test]
    fn stream_redirection_changes_action_digest() {
        let base = action(OutputSymlinkMode::default());
        let mut with_stdout = base.clone();
        if let Action::RunCommand { stdout_path, .. } = &mut with_stdout {
            *stdout_path = Some(Box::new(WorkspacePath::try_from("out.log").unwrap()));
        }
        let mut with_stderr = base.clone();
        if let Action::RunCommand { stderr_path, .. } = &mut with_stderr {
            *stderr_path = Some(Box::new(WorkspacePath::try_from("out.log").unwrap()));
        }
        assert_ne!(base.digest(), with_stdout.digest());
        assert_ne!(base.digest(), with_stderr.digest());
        assert_ne!(with_stdout.digest(), with_stderr.digest());
    }
}
