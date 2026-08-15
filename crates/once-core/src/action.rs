use std::collections::BTreeMap;
use std::path::PathBuf;
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
pub(crate) const ACTION_DIGEST_DOMAIN: &[u8] = b"once.action.v11\0";

static DEFAULT_RESOURCE_REQUEST: LazyLock<ResourceRequest> =
    LazyLock::new(ResourceRequest::default);

fn default_success_exit_codes() -> Vec<i32> {
    vec![0]
}

fn is_default_success_exit_codes(codes: &[i32]) -> bool {
    codes == [0]
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
/// What happens to a symbolic link found among an action's outputs.
pub enum OutputSymlinkMode {
    Preserve,
    #[default]
    MaterializeExternal,
}

impl OutputSymlinkMode {
    /// True when unchanged from the default, so serialization can skip it
    /// and leave existing action digests stable.
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
/// How strongly an action is isolated from the workspace while it runs.
pub enum SandboxMode {
    #[default]
    Off,
    Inputs,
    CopiedInputs,
}

impl SandboxMode {
    /// True when unchanged from the default, so serialization can skip it
    /// and leave existing action digests stable.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The stricter of two modes. Combining requirements must never
    /// weaken isolation, so this takes the maximum rather than the last
    /// value set.
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
            "copied-inputs" => Ok(Self::CopiedInputs),
            _ => Err(format!(
                "expected `off`, `inputs`, or `copied-inputs`, got `{raw}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
/// Whether an action may reach the network while it runs.
///
/// The default leaves the host network available, matching the behavior
/// scripts have always had, so adding the field never breaks an existing
/// action. `Deny` declares that the action must not use the network, and the
/// executor enforces it where the platform allows (an empty network
/// namespace on Linux). The eventual goal is default-deny; the staged
/// default keeps the change non-breaking while the plumbing lands.
pub enum NetworkPolicy {
    #[default]
    Unrestricted,
    Deny,
}

impl NetworkPolicy {
    /// True when unchanged from the default, so serialization can skip it
    /// and leave existing action digests stable.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// True when the action declares it must not use the network.
    #[must_use]
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny)
    }
}

impl FromStr for NetworkPolicy {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "unrestricted" => Ok(Self::Unrestricted),
            "deny" => Ok(Self::Deny),
            _ => Err(format!("expected `unrestricted` or `deny`, got `{raw}`")),
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
        /// Whether the action may reach the network. Defaults to
        /// unrestricted to stay non-breaking; `Deny` asks the executor to
        /// isolate the child from the network. Part of the action digest so
        /// two runs that differ only in network policy never share a cache
        /// slot.
        #[serde(default, skip_serializing_if = "NetworkPolicy::is_default")]
        network: NetworkPolicy,
        /// Per-action timeout in milliseconds. None = no timeout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        /// Exit codes that mean the command completed successfully. Linters
        /// commonly use a nonzero code to report findings while still
        /// producing a valid machine-readable result.
        #[serde(
            default = "default_success_exit_codes",
            skip_serializing_if = "is_default_success_exit_codes"
        )]
        success_exit_codes: Vec<i32>,
        /// Optional compute provider for remote execution. This is
        /// part of the action key so local and remote runs never share
        /// a cache slot by accident.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<Box<RemoteExecution>>,
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
    LinkPath {
        source: WorkspacePath,
        destination: WorkspacePath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
    MaterializeHostFile {
        source: PathBuf,
        source_sha256: String,
        destination: WorkspacePath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
    MaterializeHostTree {
        source: PathBuf,
        source_sha256: String,
        destination: WorkspacePath,
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
    WriteArchive {
        entries: Vec<ArchiveEntry>,
        output: WorkspacePath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256_output: Option<WorkspacePath>,
        format: ArchiveFormat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Container format an archive action writes.
pub enum ArchiveFormat {
    /// Uncompressed tar.
    Tar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// One member of an archive.
///
/// Ownership, modes, and mtime are declared rather than copied from disk
/// so the same inputs archive identically on any machine.
pub struct ArchiveEntry {
    /// Whether this entry is a file, a directory, or a whole subtree.
    pub kind: ArchiveEntryKind,
    /// Workspace path the contents come from. Absent for an entry the
    /// archive synthesises, such as a bare directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<WorkspacePath>,
    /// Path recorded inside the archive.
    pub path: String,
    /// Permission bits for a file entry.
    pub mode: u32,
    /// Permission bits for a directory entry.
    pub directory_mode: u32,
    /// Owner id recorded in the archive.
    pub owner_id: u64,
    /// Group id recorded in the archive.
    pub group_id: u64,
    /// Modification time recorded in the archive, in seconds.
    pub mtime: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Shape of an [`ArchiveEntry`].
pub enum ArchiveEntryKind {
    /// A single file.
    File,
    /// A directory entry with no contents of its own.
    Directory,
    /// A directory and everything beneath it.
    Tree,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Whether a copy action moves one file or a whole subtree.
pub enum CopyPathMode {
    /// Copy a single file.
    File,
    /// Copy a directory and everything beneath it.
    Tree,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// What preparing a path does to it before an action runs.
pub enum PreparePathMode {
    /// Delete whatever is there.
    Remove,
    /// Ensure a directory exists there.
    Directory,
}

impl Action {
    /// Whether this exit code counts as success. Actions declare their
    /// own accepted codes, since tools like linters use a nonzero exit to
    /// report findings rather than failure.
    #[must_use]
    pub fn accepts_exit_code(&self, exit_code: i32) -> bool {
        match self {
            Self::RunCommand {
                success_exit_codes, ..
            } => success_exit_codes.contains(&exit_code),
            _ => exit_code == 0,
        }
    }

    /// Canonical, content-addressed key for this action.
    ///
    /// The key is `BLAKE3(domain || canonical_json(self))`. Bumping the
    /// domain partitions old and new cache entries cleanly instead of
    /// silently colliding.
    pub fn digest(&self) -> Digest {
        let body = serde_json::to_vec(self).expect("Action is serializable");
        let domain = match self {
            Action::CopyPath { .. } => b"once.action.copy_path.v2\0".as_slice(),
            _ => ACTION_DIGEST_DOMAIN,
        };
        let mut buf = Vec::with_capacity(domain.len() + body.len());
        buf.extend_from_slice(domain);
        buf.extend_from_slice(&body);
        Digest::of_bytes(&buf)
    }

    pub fn resource_request(&self) -> &ResourceRequest {
        match self {
            Action::RunCommand { resources, .. } => resources,
            Action::WriteFile { .. }
            | Action::CopyPath { .. }
            | Action::LinkPath { .. }
            | Action::MaterializeHostFile { .. }
            | Action::MaterializeHostTree { .. }
            | Action::PreparePath { .. }
            | Action::WriteTreeDigest { .. }
            | Action::WriteArchive { .. } => &DEFAULT_RESOURCE_REQUEST,
        }
    }

    pub fn input_digest(&self) -> Option<Digest> {
        match self {
            Action::RunCommand { input_digest, .. }
            | Action::WriteFile { input_digest, .. }
            | Action::CopyPath { input_digest, .. }
            | Action::LinkPath { input_digest, .. }
            | Action::MaterializeHostFile { input_digest, .. }
            | Action::MaterializeHostTree { input_digest, .. }
            | Action::PreparePath { input_digest, .. }
            | Action::WriteTreeDigest { input_digest, .. }
            | Action::WriteArchive { input_digest, .. } => *input_digest,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Where an action runs when it is not run locally.
pub struct RemoteExecution {
    /// Named infrastructure configuration used for placement and logs.
    pub provider: String,
    /// Executor adapter. When absent, the provider name identifies the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// Immutable toolchain image, snapshot, or template used for this action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Account the remote work is billed and scoped to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Project the remote work is scoped to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl RemoteExecution {
    /// Target `provider` with every other field left at its default.
    pub fn provider(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            executor: None,
            environment: None,
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
            network: NetworkPolicy::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
    fn sandbox_modes_parse_from_their_public_names() {
        assert_eq!("off".parse(), Ok(SandboxMode::Off));
        assert_eq!("inputs".parse(), Ok(SandboxMode::Inputs));
        assert_eq!("copied-inputs".parse(), Ok(SandboxMode::CopiedInputs));
    }

    #[test]
    fn network_policies_parse_from_their_public_names() {
        assert_eq!("unrestricted".parse(), Ok(NetworkPolicy::Unrestricted));
        assert_eq!("deny".parse(), Ok(NetworkPolicy::Deny));
        assert!("open".parse::<NetworkPolicy>().is_err());
    }

    #[test]
    fn default_network_policy_keeps_action_digest_stable() {
        // A RunCommand whose network policy is the default must serialize
        // identically to before the field existed, so old cache entries
        // stay valid.
        let base = Action::RunCommand {
            argv: vec!["true".to_string()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            network: NetworkPolicy::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        // Snapshot taken before the network field landed on this digest
        // domain. Updating the domain above invalidates this value.
        let expected = "a3b89e9c8e49eff501798b83efe755f91af14a6d57eadd3f2292775815b83255";
        assert_eq!(base.digest().to_string(), expected);
    }

    #[test]
    fn deny_network_policy_changes_action_digest() {
        let base = action(OutputSymlinkMode::default());
        let mut denied = base.clone();
        if let Action::RunCommand { network, .. } = &mut denied {
            *network = NetworkPolicy::Deny;
        }
        assert_ne!(base.digest(), denied.digest());
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

    #[test]
    fn accepted_exit_codes_change_action_digest() {
        let base = action(OutputSymlinkMode::default());
        let mut accepts_findings = base.clone();
        if let Action::RunCommand {
            success_exit_codes, ..
        } = &mut accepts_findings
        {
            *success_exit_codes = vec![0, 1];
        }
        assert_ne!(base.digest(), accepts_findings.digest());
    }

    #[test]
    fn remote_environment_changes_action_digest() {
        let mut first = action(OutputSymlinkMode::default());
        let mut second = first.clone();
        if let Action::RunCommand { remote, .. } = &mut first {
            *remote = Some(Box::new(RemoteExecution {
                provider: "remote_tests".to_string(),
                executor: Some("microsandbox".to_string()),
                environment: Some("node:22.18.0-alpine".to_string()),
                account: None,
                project: None,
            }));
        }
        if let Action::RunCommand { remote, .. } = &mut second {
            *remote = Some(Box::new(RemoteExecution {
                provider: "remote_tests".to_string(),
                executor: Some("microsandbox".to_string()),
                environment: Some("node:24.4.1-alpine".to_string()),
                account: None,
                project: None,
            }));
        }

        assert_ne!(first.digest(), second.digest());
    }
}
