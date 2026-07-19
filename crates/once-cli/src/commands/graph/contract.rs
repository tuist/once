use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use once_cas::CacheProvider;
use once_core::SandboxMode;
use serde::Serialize;

use super::analysis::BuildSession;

#[derive(Debug, Serialize)]
pub struct ActionContractValidation {
    pub valid: bool,
    pub target: String,
    pub capability: String,
    pub actions_run: usize,
    pub actions: Vec<ActionValidationSummary>,
    pub diagnostics: Vec<once_frontend::Diagnostic>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ActionValidationSummary {
    pub index: usize,
    pub identifier: String,
    pub valid: bool,
    pub exit_code: i32,
}

pub async fn validate_action_contracts(
    workspace: &Path,
    cache: &CacheProvider,
    target_id: &str,
    capability: &str,
    selected_index: Option<usize>,
) -> Result<ActionContractValidation> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let session = BuildSession::new(workspace, cache, graph, SandboxMode::Off).await?;
    let target = session.target(target_id)?;
    super::ensure_capability(target, capability)?;
    let validations = session
        .validate_actions(target, capability, selected_index)
        .await?;
    let mut diagnostics = Vec::new();
    let mut limitations = BTreeSet::new();
    let mut actions = Vec::new();
    for validation in validations {
        diagnostics.extend(validation.diagnostics);
        limitations.extend(validation.limitations);
        actions.push(ActionValidationSummary {
            index: validation.index,
            identifier: validation.identifier,
            valid: validation.valid,
            exit_code: validation.exit_code,
        });
    }
    if selected_index.is_none() && actions.len() > 1 {
        limitations.insert(
            "Actions are probed independently, so outputs from an earlier probe are not materialized for a later action"
                .to_string(),
        );
    }
    Ok(ActionContractValidation {
        valid: diagnostics.is_empty() && actions.iter().all(|action| action.valid),
        target: target.label.id.clone(),
        capability: capability.to_string(),
        actions_run: actions.len(),
        actions,
        diagnostics,
        limitations: limitations.into_iter().collect(),
    })
}
