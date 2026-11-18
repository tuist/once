//! Per-execution input sandboxes for local command actions.
//!
//! Each execution stages its inputs into a fresh directory under
//! `.struo/sandboxes`, named after the action digest plus a unique suffix, so
//! two concurrent runs of the same action never share, clear, or remove each
//! other's execroot. The directory is owned by a [`SandboxDir`] guard that
//! moves into whichever blocking task is using it, so cleanup only runs once
//! that work has stopped, even when the awaiting future is cancelled. The
//! guard and every blocking task also own a clone of the action's admission
//! lease, so a cancelled action keeps its reservation until its filesystem
//! work and cleanup have finished.

use std::path::{Path, PathBuf};
use std::time::Duration;

use struo_cas::{ActionResult, CacheProvider};

use super::{
    copy_sandbox_output_blocking, create_symlink_blocking, remove_path_blocking, spawn_admitted,
};
use crate::stream::ActionOutputObserver;
use crate::{
    contract, local, outputs, Action, Error, ExecutionPermit, OutputSymlinkMode, Result,
    WorkspacePath,
};

/// The filesystem contract a sandboxed run is held to.
///
/// Separate from the command itself because these describe the policy
/// around the run rather than what is executed, and because three bare
/// `bool`s in a positional argument list were unreadable at the call
/// site.
pub(super) struct SandboxPolicy<'a> {
    /// Paths the action declared it reads.
    pub inputs: &'a [WorkspacePath],
    /// Paths the action declared it writes.
    pub outputs: &'a [WorkspacePath],
    pub output_symlink_mode: OutputSymlinkMode,
    /// Copy inputs into the execroot instead of linking them.
    pub copy_inputs: bool,
    /// Audit the execroot against the declaration once the child exits.
    pub validate_contract: bool,
    /// The action's admission, held by the sandbox's blocking work.
    pub lease: Option<&'a ExecutionPermit>,
}

pub(super) async fn execute_sandboxed_command(
    action: &Action,
    invocation: local::Invocation<'_>,
    policy: SandboxPolicy<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    observer: Option<&dyn ActionOutputObserver>,
) -> Result<ActionResult> {
    let sandbox = prepare_input_sandbox(action, &policy, invocation.cwd, workspace_root).await?;
    let execroot = sandbox.execroot.clone();
    let result = if let Some(observer) = observer {
        local::execute_command_observed(invocation, &execroot, cache, stream_to_parent, observer)
            .await
    } else if stream_to_parent {
        local::execute_command_streaming(invocation, &execroot, cache).await
    } else {
        local::execute_command(invocation, &execroot, cache).await
    };

    let mut result = result?;
    if policy.validate_contract {
        let violations = contract::audit_filesystem(
            &execroot,
            workspace_root,
            policy.inputs,
            policy.outputs,
            &sandbox.before_execroot,
            &sandbox.before_workspace,
            &result,
            cache,
        )
        .await
        .map_err(|source| Error::FileAction {
            action: "audit_sandbox",
            path: execroot.display().to_string(),
            source,
        })?;
        if !violations.is_empty() {
            return Err(Error::ContractViolation { violations });
        }
        return Ok(result);
    }
    if action.accepts_exit_code(result.exit_code) {
        let output_paths = policy.outputs.to_vec();
        let output_symlink_mode = policy.output_symlink_mode;
        let capture_root = execroot.clone();
        let cache = cache.clone();
        let lease = policy.lease.cloned();
        let permit = crate::resource_scope::current();
        // Capture workers may outlive a cancelled caller. Keep their execroot
        // and admission alive until capture finishes, before publishing anything.
        let capture = async move {
            let captured = outputs::capture_admitted(
                &output_paths,
                &capture_root,
                &cache,
                output_symlink_mode,
                lease.as_ref(),
            )
            .await;
            (sandbox.dir, captured)
        };
        let (dir, captured) = tokio::spawn(async move {
            match permit {
                Some(permit) => crate::with_resource_permit((*permit).clone(), capture).await,
                None => capture.await,
            }
        })
        .await
        .map_err(|source| Error::FileAction {
            action: "capture_sandbox_outputs",
            path: execroot.display().to_string(),
            source: std::io::Error::other(source.to_string()),
        })?;
        result.outputs = captured?;
        copy_sandbox_outputs(policy.outputs, dir, workspace_root, policy.lease).await?;
    }
    Ok(result)
}

/// Owns one execution's sandbox directory and removes it when dropped.
///
/// Dropping it inside a runtime hands removal to the blocking pool so an async
/// worker never walks a tree; blocking code that owns it calls
/// [`SandboxDir::remove_now`] instead.
struct SandboxDir {
    root: Option<PathBuf>,
    keep: bool,
    lease: Option<ExecutionPermit>,
}

impl SandboxDir {
    fn create(
        sandboxes: &Path,
        digest: &str,
        keep: bool,
        lease: Option<ExecutionPermit>,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(sandboxes)?;
        let root = tempfile::Builder::new()
            .prefix(&format!("{digest}-"))
            .tempdir_in(sandboxes)?
            .keep();
        Ok(Self {
            root: Some(root),
            keep,
            lease,
        })
    }

    fn root(&self) -> &Path {
        self.root
            .as_deref()
            .expect("sandbox root is present until drop")
    }

    /// Remove the sandbox on the current thread. For blocking code that
    /// already owns it.
    fn remove_now(mut self) {
        if let Some(root) = self.root.take() {
            if !self.keep {
                remove_sandbox(&root);
            }
        }
    }
}

impl Drop for SandboxDir {
    fn drop(&mut self) {
        let Some(root) = self.root.take() else {
            return;
        };
        if self.keep {
            tracing::info!(sandbox = %root.display(), "keeping action sandbox");
            return;
        }
        let lease = self.lease.take();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => drop(handle.spawn_blocking(move || {
                let _lease = lease;
                remove_sandbox(&root);
            })),
            Err(_) => remove_sandbox(&root),
        }
    }
}

/// Remove a sandbox tree. Processes killed with their action can still be
/// exiting when this runs, so a failure gets a few short retries.
fn remove_sandbox(root: &Path) {
    let mut attempt = 0;
    loop {
        match remove_path_blocking(root) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) if attempt >= 3 => {
                tracing::warn!(
                    sandbox = %root.display(),
                    %error,
                    "failed to remove action sandbox"
                );
                return;
            }
            Err(_) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

struct PreparedSandbox {
    dir: SandboxDir,
    execroot: PathBuf,
    before_execroot: contract::ContractSnapshot,
    before_workspace: contract::ContractSnapshot,
}

async fn prepare_input_sandbox(
    action: &Action,
    policy: &SandboxPolicy<'_>,
    cwd: Option<&WorkspacePath>,
    workspace_root: &Path,
) -> Result<PreparedSandbox> {
    let SandboxPolicy {
        inputs,
        outputs,
        copy_inputs,
        validate_contract,
        lease,
        ..
    } = *policy;
    let sandboxes = workspace_root.join(".struo").join("sandboxes");
    let digest = action.digest().to_string();
    let input_paths = inputs.to_vec();
    let output_paths = outputs.to_vec();
    let cwd_path = cwd.cloned();
    let workspace = workspace_root.to_path_buf();
    let keep = keep_sandbox();
    // Contract validation stages copies rather than symlinks so a probe that
    // writes one of its declared inputs mutates the private execroot copy (where
    // the audit still flags the write) instead of reaching through a symlink and
    // corrupting the real workspace source the docs promise to leave untouched.
    let copy_inputs = validate_contract || copy_inputs;
    let staging_root = sandboxes.clone();
    let dir_lease = lease.cloned();
    let dir = spawn_admitted(lease, move || {
        let dir = SandboxDir::create(&staging_root, &digest, keep, dir_lease)?;
        let execroot = dir.root().join("execroot");
        let staged = (|| {
            std::fs::create_dir_all(&execroot)?;
            for input in &input_paths {
                stage_sandbox_input_blocking(&workspace, &execroot, input, copy_inputs)?;
            }
            for output in &output_paths {
                if let Some(parent) = output.resolve(&execroot).parent() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            if let Some(cwd) = &cwd_path {
                std::fs::create_dir_all(cwd.resolve(&execroot))?;
            }
            if copy_inputs {
                let keep_writable = output_paths
                    .iter()
                    .map(|output| PathBuf::from(output.as_str()))
                    .chain(cwd_path.iter().map(|cwd| PathBuf::from(cwd.as_str())))
                    .collect::<Vec<_>>();
                mirror_ancestor_permissions(&workspace, &execroot, &input_paths, &keep_writable)?;
            }
            Ok::<_, std::io::Error>(())
        })();
        match staged {
            Ok(()) => Ok(dir),
            Err(error) => {
                dir.remove_now();
                Err(error)
            }
        }
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "prepare_sandbox",
        path: sandboxes.display().to_string(),
        source: std::io::Error::other(source.to_string()),
    })?
    .map_err(|source| Error::FileAction {
        action: "prepare_sandbox",
        path: sandboxes.display().to_string(),
        source,
    })?;
    let execroot = dir.root().join("execroot");

    let before_execroot = if validate_contract {
        contract::snapshot_tree(&execroot, &[]).map_err(|source| Error::FileAction {
            action: "snapshot_sandbox",
            path: execroot.display().to_string(),
            source,
        })?
    } else {
        contract::ContractSnapshot(std::collections::BTreeMap::new())
    };
    let before_workspace = if validate_contract {
        contract::snapshot_tree(workspace_root, &[".struo"]).map_err(|source| {
            Error::FileAction {
                action: "snapshot_workspace",
                path: workspace_root.display().to_string(),
                source,
            }
        })?
    } else {
        contract::ContractSnapshot(std::collections::BTreeMap::new())
    };
    Ok(PreparedSandbox {
        dir,
        execroot,
        before_execroot,
        before_workspace,
    })
}

fn keep_sandbox() -> bool {
    std::env::var("STRUO_KEEP_SANDBOX")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
}

fn stage_sandbox_input_blocking(
    workspace_root: &Path,
    sandbox_execroot: &Path,
    input: &WorkspacePath,
    copy: bool,
) -> std::io::Result<()> {
    let source = input.resolve(workspace_root);
    let destination = input.resolve(sandbox_execroot);
    if copy {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        remove_path_blocking(&destination)?;
        return copy_sandbox_output_blocking(&source, &destination);
    }
    let metadata = std::fs::symlink_metadata(&source)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        stage_sandbox_directory_blocking(&source, &destination)
    } else {
        link_sandbox_input_blocking(&source, &destination)
    }
}

fn stage_sandbox_directory_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    let mut children = std::fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let child_source = child.path();
        let child_destination = destination.join(child.file_name());
        let metadata = std::fs::symlink_metadata(&child_source)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            stage_sandbox_directory_blocking(&child_source, &child_destination)?;
        } else {
            link_sandbox_input_blocking(&child_source, &child_destination)?;
        }
    }
    Ok(())
}

fn link_sandbox_input_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_blocking(destination)?;
    create_symlink_blocking(source, destination, source)
}

/// Copy declared outputs back into the workspace. Takes ownership of the
/// sandbox so it is removed only after the copy finishes, even if the caller
/// stops waiting for it.
async fn copy_sandbox_outputs(
    outputs: &[WorkspacePath],
    sandbox: SandboxDir,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let output_paths = outputs.to_vec();
    let execroot = sandbox.root().join("execroot");
    let label = execroot.display().to_string();
    let workspace = workspace_root.to_path_buf();
    spawn_admitted(lease, move || {
        let copied = output_paths.iter().try_for_each(|output| {
            let source = output.resolve(&execroot);
            copy_sandbox_output_blocking(&source, &output.resolve(&workspace))
        });
        sandbox.remove_now();
        copied
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "copy_sandbox_outputs",
        path: label.clone(),
        source: std::io::Error::other(source.to_string()),
    })?
    .map_err(|source| Error::FileAction {
        action: "copy_sandbox_outputs",
        path: label,
        source,
    })
}

/// Give the directories above each staged input the permissions they have in
/// the workspace. Directories created on the way to an input otherwise take
/// default modes, so a `0700` directory would arrive as `0755`. Runs after
/// every input is staged, deepest directories first, so a restrictive parent
/// cannot block staging its siblings.
fn mirror_ancestor_permissions(
    workspace_root: &Path,
    sandbox_execroot: &Path,
    inputs: &[WorkspacePath],
    keep_writable: &[PathBuf],
) -> std::io::Result<()> {
    let mut directories = std::collections::BTreeSet::new();
    for input in inputs {
        for ancestor in Path::new(input.as_str()).ancestors().skip(1) {
            if !ancestor.as_os_str().is_empty() {
                directories.insert(ancestor.to_path_buf());
            }
        }
    }
    let mut ordered = directories.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|directory| std::cmp::Reverse(directory.components().count()));
    for directory in ordered {
        if keep_writable
            .iter()
            .any(|kept| kept.starts_with(&directory))
        {
            continue;
        }
        let source = std::fs::symlink_metadata(workspace_root.join(&directory));
        let destination = std::fs::symlink_metadata(sandbox_execroot.join(&directory));
        if let (Ok(source), Ok(destination)) = (source, destination) {
            if source.is_dir()
                && destination.is_dir()
                && !source.file_type().is_symlink()
                && !destination.file_type().is_symlink()
            {
                std::fs::set_permissions(sandbox_execroot.join(&directory), source.permissions())?;
            }
        }
    }
    Ok(())
}
