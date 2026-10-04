//! `once connect` - provision a project with a provider and bind it.
//!
//! The command is provider-neutral: it resolves the named provider, asks it to
//! provision or list a project, and records the resulting binding in the root
//! `once.toml`. Provider protocol details, including how a project is created,
//! live behind [`crate::provision`]. This is the human-facing surface; the same
//! binding write can be reproduced by hand from the resulting `once.toml`.

use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use once_core::Xdg;
use once_frontend::{apply_infrastructure_binding, InfrastructureBinding};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::provision::{self, CreateProjectError, ProjectRef};
use crate::render;

/// Arguments collected from the `once connect` command.
#[derive(Debug, Clone)]
pub struct ConnectArgs {
    /// Provider reference, resolved like `once auth login --provider`.
    pub provider: String,
    /// Account or organization handle that owns the project.
    pub account: Option<String>,
    /// Project handle.
    pub project: Option<String>,
    /// Create the project when it does not exist instead of only binding it.
    pub create: bool,
    /// Print the binding without writing `once.toml` or mutating remote state.
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ConnectResult {
    Connected {
        provider: String,
        server: String,
        account: String,
        project: String,
        full_handle: String,
        binding_path: String,
        changed: bool,
    },
    Planned {
        provider: String,
        server: String,
        account: String,
        project: String,
        full_handle: String,
        changed: bool,
    },
}

pub async fn connect(workspace: &Path, xdg: &Xdg, output: Output, args: ConnectArgs) -> Result<()> {
    if args.account.is_some() != args.project.is_some() {
        bail!("`--account` and `--project` must be provided together");
    }

    let provisioner = provision::resolve(workspace, xdg, &args.provider)?;
    let provider_name = provisioner.provider_name().to_string();
    let server = provisioner.server_url().to_string();
    let manifest_path = workspace.join(once_frontend::TOML_BUILD_FILE_NAME);
    let existing = read_manifest(&manifest_path)?;

    if args.dry_run {
        let planned = plan_binding(&provisioner, workspace, &args).await?;
        let binding = binding_for(&provisioner, &planned);
        let updated = plan_write(&manifest_path, &existing, &binding)?;
        let full_handle = planned.full_handle();
        let result = ConnectResult::Planned {
            provider: provider_name,
            server,
            account: planned.account,
            project: planned.project,
            full_handle,
            changed: updated != existing,
        };
        return write_body(output, &result).await;
    }

    // A re-run must converge rather than create a second project, so resolve the
    // binding already in the workspace before touching the provider. Only a
    // manifest that is already canonical can short-circuit; an inline provider
    // binding still surfaces its conflict below.
    if let Some(existing_binding) = existing_workspace_binding(workspace, xdg) {
        if binding_matches(&existing_binding, workspace, &args)? {
            let canonical = binding_for(&provisioner, &existing_binding);
            let unchanged = plan_write(&manifest_path, &existing, &canonical)
                .is_ok_and(|updated| updated == existing);
            if unchanged {
                return finish(output, &provisioner, existing_binding, manifest_path, false).await;
            }
        }
    }

    if !provisioner.has_session() {
        // Reuse the existing login flow rather than a second auth entry point,
        // so `once connect` is a single step after the provider is chosen.
        crate::commands::auth::login(workspace, xdg, &args.provider, true, output).await?;
    }

    // Structural manifest failures are cheap to detect and expensive to
    // discover after a remote project exists, so prove the write first.
    let preflight = binding_for(&provisioner, &placeholder_ref(workspace, &args)?);
    plan_write(&manifest_path, &existing, &preflight)?;

    let selected = select_project(&provisioner, workspace, &args).await?;
    let binding = binding_for(&provisioner, &selected);

    // Optimistic concurrency: refuse to clobber a manifest edited while the
    // provider call was in flight.
    let current = read_manifest(&manifest_path)?;
    if current != existing {
        bail!(
            "`{}` changed while connecting; re-run `once connect` to apply the binding",
            manifest_path.display()
        );
    }
    let updated = plan_write(&manifest_path, &existing, &binding)?;
    let changed = updated != existing;
    if changed {
        write_atomic(&manifest_path, &updated).with_context(|| {
            format!(
                "the provider project `{}` was created but the binding could not be written; \
                 re-run `once connect` to record it",
                selected.full_handle()
            )
        })?;
    }
    finish(output, &provisioner, selected, manifest_path, changed).await
}

async fn finish(
    output: Output,
    provisioner: &provision::Provisioner,
    selected: ProjectRef,
    manifest_path: std::path::PathBuf,
    changed: bool,
) -> Result<()> {
    let full_handle = selected.full_handle();
    let result = ConnectResult::Connected {
        provider: provisioner.provider_name().to_string(),
        server: provisioner.server_url().to_string(),
        account: selected.account,
        project: selected.project,
        full_handle,
        binding_path: manifest_path.to_string_lossy().into_owned(),
        changed,
    };
    write_body(output, &result).await
}

fn binding_for(
    provisioner: &provision::Provisioner,
    project: &ProjectRef,
) -> InfrastructureBinding {
    InfrastructureBinding {
        provider_name: provisioner.provider_name().to_string(),
        kind: provisioner.kind().to_string(),
        url: provisioner.server_url().to_string(),
        account: project.account.clone(),
        project: project.project.clone(),
        oauth_client_id: provisioner.oauth_client_id().map(str::to_string),
    }
}

/// Resolve the project a dry run would bind, without mutating remote state.
async fn plan_binding(
    provisioner: &provision::Provisioner,
    workspace: &Path,
    args: &ConnectArgs,
) -> Result<ProjectRef> {
    if args.create {
        let project = match args.project.as_deref() {
            Some(project) => project.to_string(),
            None => default_project_name(workspace)?,
        };
        let account = args.account.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "`--dry-run --create` needs an explicit `--account` so the binding can be resolved \
                 without creating the project"
            )
        })?;
        return Ok(ProjectRef { account, project });
    }
    select_existing(provisioner, args).await
}

/// A placeholder binding used to prove the manifest edit before a remote
/// project exists. Only the structural checks depend on these values.
fn placeholder_ref(workspace: &Path, args: &ConnectArgs) -> Result<ProjectRef> {
    Ok(ProjectRef {
        account: args
            .account
            .clone()
            .unwrap_or_else(|| "pending-account".to_string()),
        project: match args.project.as_deref() {
            Some(project) => project.to_string(),
            None => default_project_name(workspace)?,
        },
    })
}

/// The project currently bound in the workspace, when the effective provider is
/// a Tuist provider.
fn existing_workspace_binding(workspace: &Path, xdg: &Xdg) -> Option<ProjectRef> {
    match crate::cache_provider::resolve_config(workspace, xdg).ok()? {
        crate::cache_provider::ResolvedCacheProviderConfig::Tuist(config) => Some(ProjectRef {
            account: config.account?,
            project: config.project?,
        }),
        crate::cache_provider::ResolvedCacheProviderConfig::Local => None,
    }
}

fn binding_matches(binding: &ProjectRef, workspace: &Path, args: &ConnectArgs) -> Result<bool> {
    if let Some(account) = args.account.as_deref() {
        if account != binding.account {
            return Ok(false);
        }
    }
    let project = match args.project.as_deref() {
        Some(project) => project.to_string(),
        None => default_project_name(workspace)?,
    };
    Ok(project == binding.project)
}

async fn select_project(
    provisioner: &provision::Provisioner,
    workspace: &Path,
    args: &ConnectArgs,
) -> Result<ProjectRef> {
    if args.create {
        return create_or_bind(provisioner, workspace, args).await;
    }
    select_existing(provisioner, args).await
}

async fn select_existing(
    provisioner: &provision::Provisioner,
    args: &ConnectArgs,
) -> Result<ProjectRef> {
    if let (Some(account), Some(project)) = (args.account.as_deref(), args.project.as_deref()) {
        let requested = ProjectRef::parse(&format!("{account}/{project}")).ok_or_else(|| {
            anyhow::anyhow!("`--account` and `--project` must both be non-empty handles")
        })?;
        let projects = provisioner.list_projects().await?;
        if projects.contains(&requested) {
            return Ok(requested);
        }
        bail!(
            "provider `{}` does not expose `{}` for this session. \
             Check the handle or run `once connect --provider {} --create --account {} --project {}`.",
            provisioner.provider_name(),
            requested.full_handle(),
            provisioner.provider_name(),
            account,
            project
        );
    }

    let projects = provisioner.list_projects().await?;
    match projects.as_slice() {
        [] => bail!(
            "provider `{}` has no projects for this session. \
             Re-run with `--create` to create one.",
            provisioner.provider_name()
        ),
        [only] => Ok(only.clone()),
        many => bail!(
            "provider `{}` has {} projects; select one with `--account <handle> --project <handle>`. \
             Candidates: {}",
            provisioner.provider_name(),
            many.len(),
            many.iter()
                .map(ProjectRef::full_handle)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

async fn create_or_bind(
    provisioner: &provision::Provisioner,
    workspace: &Path,
    args: &ConnectArgs,
) -> Result<ProjectRef> {
    let project = match args.project.as_deref() {
        Some(project) => project.to_string(),
        None => default_project_name(workspace)?,
    };
    match provisioner
        .create_project(args.account.as_deref(), &project)
        .await
    {
        Ok(project) => Ok(project),
        Err(CreateProjectError::AlreadyExists(_)) => {
            // The provider committed the project before, or another session
            // owns the handle. Resolve the exact account instead of guessing.
            if let Some(account) = args.account.as_deref() {
                return Ok(ProjectRef {
                    account: account.to_string(),
                    project,
                });
            }
            let matches = provisioner
                .list_projects()
                .await?
                .into_iter()
                .filter(|existing| existing.project == project)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [only] => Ok(only.clone()),
                [] => bail!(
                    "provider `{}` reported `{project}` already exists but it is not visible to this session",
                    provisioner.provider_name()
                ),
                many => bail!(
                    "provider `{}` has multiple projects named `{project}`; create or bind one with \
                     `--account <handle> --project <handle>`. Candidates: {}",
                    provisioner.provider_name(),
                    many.iter()
                        .map(ProjectRef::full_handle)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }
        Err(CreateProjectError::PermissionDenied) => bail!(
            "the connected session is not allowed to create projects with provider `{}`. \
             Re-run `once auth login --provider {}` to grant project creation, then retry.",
            provisioner.provider_name(),
            provisioner.provider_name()
        ),
        Err(CreateProjectError::Other(error)) => Err(error),
    }
}

fn default_project_name(workspace: &Path) -> Result<String> {
    let name = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .context("could not derive a project name from the workspace directory")?;
    Ok(name.to_string())
}

/// Apply and parse the binding without writing, returning the new body.
fn plan_write(path: &Path, existing: &str, binding: &InfrastructureBinding) -> Result<String> {
    let updated = apply_infrastructure_binding(existing, binding)
        .map_err(|diagnostics| diagnostics_error(&diagnostics))?;
    once_frontend::load_infrastructure_toml_str(&path.to_string_lossy(), &updated)
        .with_context(|| format!("validating the updated `{}`", path.display()))?;
    Ok(updated)
}

fn read_manifest(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("reading `{}`", path.display())),
    }
}

fn write_atomic(path: &Path, body: &str) -> Result<()> {
    let directory = path
        .parent()
        .context("`once.toml` path had no parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)
        .with_context(|| format!("creating a temporary file next to `{}`", path.display()))?;
    temp.write_all(body.as_bytes())
        .with_context(|| format!("writing `{}`", path.display()))?;
    // Preserve the existing file mode when replacing it.
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = temp.as_file().set_permissions(metadata.permissions());
    }
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replacing `{}`", path.display()))?;
    Ok(())
}

fn diagnostics_error(diagnostics: &[once_frontend::Diagnostic]) -> anyhow::Error {
    let mut message = String::from("could not record the provider binding:");
    for diagnostic in diagnostics {
        message.push_str("\n  ");
        message.push_str(&diagnostic.code);
        message.push_str(": ");
        message.push_str(&diagnostic.message);
        for repair in &diagnostic.repairs {
            message.push_str("\n    - ");
            message.push_str(repair);
        }
    }
    anyhow::anyhow!(message)
}

async fn write_body<T: Serialize>(output: Output, value: &T) -> Result<()> {
    let body = match output.format {
        Format::Human => render_human(value),
        Format::Json | Format::Toon => render::structured(output.format, value)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn render_human<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value).ok() {
        Some(serde_json::Value::Object(map)) => {
            let full_handle = map
                .get("full_handle")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            match map.get("status").and_then(serde_json::Value::as_str) {
                Some("connected") => {
                    let changed = map
                        .get("changed")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);
                    let verb = if changed {
                        "connected"
                    } else {
                        "already connected to"
                    };
                    format!("{verb} {full_handle}\n")
                }
                Some("planned") => {
                    let changed = map
                        .get("changed")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);
                    if changed {
                        format!("would connect {full_handle}\n")
                    } else {
                        format!("already connected to {full_handle}\n")
                    }
                }
                _ => format!("{full_handle}\n"),
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_matches_project_and_account() {
        let binding = ProjectRef {
            account: "acme".to_string(),
            project: "app".to_string(),
        };
        let workspace = Path::new("/tmp/app");
        let args = ConnectArgs {
            provider: "tuist".to_string(),
            account: Some("acme".to_string()),
            project: Some("app".to_string()),
            create: true,
            dry_run: false,
        };
        assert!(binding_matches(&binding, workspace, &args).unwrap());

        let other = ConnectArgs {
            account: Some("other".to_string()),
            ..args
        };
        assert!(!binding_matches(&binding, workspace, &other).unwrap());
    }

    #[test]
    fn binding_matches_defaults_project_from_workspace_name() {
        let binding = ProjectRef {
            account: "acme".to_string(),
            project: "app".to_string(),
        };
        let args = ConnectArgs {
            provider: "tuist".to_string(),
            account: None,
            project: None,
            create: true,
            dry_run: false,
        };
        assert!(binding_matches(&binding, Path::new("/tmp/app"), &args).unwrap());
        assert!(!binding_matches(&binding, Path::new("/tmp/other"), &args).unwrap());
    }
}
