use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};
use serde::{Deserialize, Serialize};

use crate::{Action, SandboxMode, WorkspacePath};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilesystemOperation {
    Read,
    Write,
    Modify,
    Delete,
    Access,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionContractDiagnostic {
    pub code: String,
    pub operation: FilesystemOperation,
    pub path: String,
    pub message: String,
    pub repairs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionContractReport {
    pub valid: bool,
    pub exit_code: i32,
    pub diagnostics: Vec<ActionContractDiagnostic>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ActionContractOptions {
    pub create_dirs: Vec<WorkspacePath>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContractViolationKind {
    UndeclaredRead,
    UndeclaredWrite,
    DeclaredInputModified,
    DeclaredInputDeleted,
    SymlinkEscape,
    MissingOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractViolation {
    pub kind: ContractViolationKind,
    pub path: String,
    pub message: String,
    pub repair: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractSnapshot(pub(crate) BTreeMap<String, String>);

pub async fn validate_action_contract(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
) -> std::io::Result<ActionContractReport> {
    validate_action_contract_with_options(
        action,
        workspace,
        cache,
        &ActionContractOptions::default(),
    )
    .await
}

pub async fn validate_action_contract_with_options(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
    options: &ActionContractOptions,
) -> std::io::Result<ActionContractReport> {
    for directory in &options.create_dirs {
        std::fs::create_dir_all(directory.resolve(workspace))?;
    }
    let mut preflight = Vec::new();
    if let Action::RunCommand {
        argv, env, inputs, ..
    } = action
    {
        let input_set = inputs
            .iter()
            .map(|path| path.as_str())
            .collect::<BTreeSet<_>>();
        let workspace_text = workspace.display().to_string();
        let command_text = argv
            .iter()
            .chain(env.values())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        if command_text.contains(&workspace_text) {
            for entry in walk_paths(workspace)? {
                if !input_set.contains(entry.as_str())
                    && command_text.contains(&workspace.join(&entry).display().to_string())
                {
                    preflight.push(ActionContractDiagnostic {
                        code: "undeclared_read".to_string(), operation: FilesystemOperation::Read,
                        path: entry.clone(), message: "action command contains an absolute workspace path outside its declared inputs".to_string(),
                        repairs: vec![format!("Add `{entry}` to the action inputs and use a workspace-relative path")],
                    });
                }
            }
        }
        for input in inputs {
            let source = input.resolve(workspace);
            if std::fs::symlink_metadata(&source)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                if let Ok(resolved) = std::fs::canonicalize(&source) {
                    let covered = inputs
                        .iter()
                        .any(|other| resolved.starts_with(other.resolve(workspace)));
                    if !covered {
                        preflight.push(ActionContractDiagnostic {
                            code: "symlink_escape".to_string(), operation: FilesystemOperation::Access,
                            path: input.as_str().to_string(), message: "declared input symlink resolves outside the declared input set".to_string(),
                            repairs: vec!["Declare the symlink target as an input or replace the symlink with a file".to_string()],
                        });
                    }
                }
            }
        }
    }
    if !preflight.is_empty() {
        return Ok(ActionContractReport {
            valid: false,
            exit_code: 0,
            diagnostics: preflight,
            limitations: vec![
                "Successful reads are not observable with a symlink-only sandbox".to_string(),
            ],
        });
    }
    let mut validated = action.clone();
    if let Action::RunCommand { sandbox, .. } = &mut validated {
        *sandbox = SandboxMode::Inputs;
    } else {
        return Ok(ActionContractReport {
            valid: true,
            exit_code: 0,
            diagnostics: Vec::new(),
            limitations: vec![
                "Only command actions have a filesystem contract to validate".to_string(),
            ],
        });
    }
    match crate::run_uncached_contract(&validated, workspace, cache, false).await {
        Ok(result) => Ok(ActionContractReport {
            valid: result.exit_code == 0,
            exit_code: result.exit_code,
            diagnostics: Vec::new(),
            limitations: vec![
                "Successful reads are not observable with a symlink-only sandbox".to_string(),
            ],
        }),
        Err(crate::Error::ContractViolation { violations }) => Ok(ActionContractReport {
            valid: false,
            exit_code: 0,
            diagnostics: violations
                .into_iter()
                .map(diagnostic_from_violation)
                .collect(),
            limitations: vec![
                "Successful reads are not observable with a symlink-only sandbox".to_string(),
            ],
        }),
        Err(error) => Err(std::io::Error::other(error.to_string())),
    }
}

fn diagnostic_from_violation(violation: ContractViolation) -> ActionContractDiagnostic {
    let (code, operation) = match violation.kind {
        ContractViolationKind::UndeclaredRead => ("undeclared_read", FilesystemOperation::Read),
        ContractViolationKind::UndeclaredWrite => ("undeclared_write", FilesystemOperation::Write),
        ContractViolationKind::DeclaredInputModified => {
            ("declared_input_modified", FilesystemOperation::Modify)
        }
        ContractViolationKind::DeclaredInputDeleted => {
            ("declared_input_deleted", FilesystemOperation::Delete)
        }
        ContractViolationKind::SymlinkEscape => ("symlink_escape", FilesystemOperation::Access),
        ContractViolationKind::MissingOutput => ("missing_output", FilesystemOperation::Write),
    };
    ActionContractDiagnostic {
        code: code.to_string(),
        operation,
        path: violation.path,
        message: violation.message,
        repairs: vec![violation.repair],
    }
}

pub(crate) fn snapshot_tree(root: &Path, excluded: &[&str]) -> std::io::Result<ContractSnapshot> {
    let mut entries = BTreeMap::new();
    if root.exists() {
        walk(root, root, excluded, &mut entries)?;
    }
    Ok(ContractSnapshot(entries))
}

pub(crate) async fn audit_filesystem(
    execroot: &Path,
    workspace: &Path,
    inputs: &[WorkspacePath],
    outputs: &[WorkspacePath],
    before_execroot: &ContractSnapshot,
    before_workspace: &ContractSnapshot,
    result: &ActionResult,
    cache: &CacheProvider,
) -> std::io::Result<Vec<ContractViolation>> {
    let allowed_outputs = outputs
        .iter()
        .map(|p| p.as_str().trim_end_matches('/').to_string())
        .collect::<BTreeSet<_>>();
    let after_execroot = snapshot_tree(execroot, &[])?;
    let mut violations = Vec::new();
    for path in changed_paths(before_execroot, &after_execroot) {
        if !under_any(&path, &allowed_outputs) && !is_output_parent(&path, &allowed_outputs) {
            violations.push(ContractViolation {
                kind: ContractViolationKind::UndeclaredWrite,
                path,
                message: "action changed a path outside its declared outputs".to_string(),
                repair: "Declare this path as an output or stop writing it".to_string(),
            });
        }
    }
    for output in outputs {
        let path = output.resolve(execroot);
        if !path.try_exists()? {
            violations.push(ContractViolation {
                kind: ContractViolationKind::MissingOutput,
                path: output.as_str().to_string(),
                message: "declared output was not produced in the private execroot".to_string(),
                repair: "Produce this output or remove it from the action outputs".to_string(),
            });
        } else {
            if std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                if let Ok(resolved) = std::fs::canonicalize(&path) {
                    if !resolved.starts_with(execroot) {
                        violations.push(ContractViolation {
                            kind: ContractViolationKind::SymlinkEscape,
                            path: output.as_str().to_string(),
                            message: "declared output resolves outside the private execroot"
                                .to_string(),
                            repair: "Write a regular output inside the action execroot".to_string(),
                        });
                    }
                }
            }
            collect_output_symlink_escapes(&path, execroot, &mut violations)?;
        }
    }
    let input_set = inputs
        .iter()
        .map(|p| p.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let after_workspace = snapshot_tree(workspace, &[".once"])?;
    for input in inputs {
        let before = before_workspace.0.get(input.as_str());
        let after = after_workspace.0.get(input.as_str());
        if before.is_some() && after.is_none() {
            violations.push(ContractViolation {
                kind: ContractViolationKind::DeclaredInputDeleted,
                path: input.as_str().to_string(),
                message: "declared input was deleted during the action".to_string(),
                repair: "Stop deleting declared inputs; write a separate output instead"
                    .to_string(),
            });
        } else if before != after {
            violations.push(ContractViolation {
                kind: ContractViolationKind::DeclaredInputModified,
                path: input.as_str().to_string(),
                message: "declared input changed during the action".to_string(),
                repair: "Stop mutating declared inputs; write a separate output instead"
                    .to_string(),
            });
        }
    }
    for path in changed_paths(before_workspace, &after_workspace) {
        if !input_set.contains(&path) && !under_any(&path, &allowed_outputs) {
            violations.push(ContractViolation {
                kind: ContractViolationKind::UndeclaredWrite,
                path,
                message: "action changed a workspace path outside its declared contract"
                    .to_string(),
                repair: "Declare this path as an output or stop accessing the real workspace"
                    .to_string(),
            });
        }
    }
    if result.exit_code != 0 {
        if let Some(stderr) = result.stderr {
            let bytes = cache
                .get_blob(&stderr)
                .await
                .map_err(std::io::Error::other)?;
            let text = String::from_utf8_lossy(&bytes);
            for path in before_workspace.0.keys() {
                if !input_set.contains(path)
                    && !under_any(path, &allowed_outputs)
                    && text.contains(path)
                {
                    violations.push(ContractViolation {
                        kind: ContractViolationKind::UndeclaredRead,
                        path: path.clone(),
                        message: "action attempted to read a workspace path that is not declared"
                            .to_string(),
                        repair: format!("Add `{path}` to the action inputs"),
                    });
                }
            }
        }
    }
    violations.sort_by(|a, b| a.path.cmp(&b.path));
    violations.dedup();
    Ok(violations)
}

fn changed_paths(before: &ContractSnapshot, after: &ContractSnapshot) -> Vec<String> {
    before
        .0
        .keys()
        .chain(after.0.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|p| before.0.get(p) != after.0.get(p))
        .collect()
}
fn under_any(path: &str, prefixes: &BTreeSet<String>) -> bool {
    prefixes
        .iter()
        .any(|p| path == p || path.starts_with(&format!("{p}/")))
}

fn collect_output_symlink_escapes(
    path: &Path,
    execroot: &Path,
    violations: &mut Vec<ContractViolation>,
) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        if let Ok(resolved) = std::fs::canonicalize(path) {
            if !resolved.starts_with(execroot) {
                violations.push(ContractViolation {
                    kind: ContractViolationKind::SymlinkEscape,
                    path: path
                        .strip_prefix(execroot)
                        .unwrap_or(path)
                        .display()
                        .to_string(),
                    message: "output symlink resolves outside the private execroot".to_string(),
                    repair: "Write a regular output inside the action execroot".to_string(),
                });
            }
        }
    } else if metadata.is_dir() {
        for child in std::fs::read_dir(path)? {
            collect_output_symlink_escapes(&child?.path(), execroot, violations)?;
        }
    }
    Ok(())
}

fn walk_paths(root: &Path) -> std::io::Result<Vec<String>> {
    let snapshot = snapshot_tree(root, &[".once"])?;
    Ok(snapshot.0.into_keys().collect())
}
fn is_output_parent(path: &str, outputs: &BTreeSet<String>) -> bool {
    outputs
        .iter()
        .any(|output| output.starts_with(&format!("{path}/")))
}
fn walk(
    root: &Path,
    current: &Path,
    excluded: &[&str],
    entries: &mut BTreeMap<String, String>,
) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(current)?;
    let relative = current
        .strip_prefix(root)
        .unwrap_or(current)
        .to_string_lossy()
        .replace('\\', "/");
    if !relative.is_empty()
        && excluded
            .iter()
            .any(|e| relative == *e || relative.starts_with(&format!("{e}/")))
    {
        return Ok(());
    }
    let fingerprint = if metadata.file_type().is_symlink() {
        format!("link:{:?}", std::fs::read_link(current)?)
    } else if metadata.is_dir() {
        "dir".to_string()
    } else if metadata.is_file() {
        format!("file:{:?}", std::fs::read(current)?)
    } else {
        "other".to_string()
    };
    if !relative.is_empty() {
        entries.insert(relative, fingerprint);
    }
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for child in std::fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()? {
            walk(root, &child.path(), excluded, entries)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_include_repairs_for_each_contract_violation() {
        let cases = [
            ContractViolationKind::UndeclaredRead,
            ContractViolationKind::UndeclaredWrite,
            ContractViolationKind::DeclaredInputModified,
            ContractViolationKind::DeclaredInputDeleted,
            ContractViolationKind::SymlinkEscape,
            ContractViolationKind::MissingOutput,
        ];
        for kind in cases {
            let diagnostic = diagnostic_from_violation(ContractViolation {
                kind,
                path: "artifact".to_string(),
                message: "observed during validation".to_string(),
                repair: "repair the action declaration".to_string(),
            });
            assert!(!diagnostic.code.is_empty());
            assert!(!diagnostic.repairs.is_empty(), "{kind:?}");
        }
    }
}
