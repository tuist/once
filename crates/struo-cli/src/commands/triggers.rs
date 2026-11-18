//! Data-only registration and simulation inside an existing CI allocation.

mod registration;
pub(crate) mod schema;
mod storage;
mod worker;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use struo_frontend::{TriggerEditOperation, TriggerSpec};
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};

pub(crate) use registration::{activate, plan};
pub(crate) use worker::run;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateInput {
    trigger: TriggerSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditInput {
    operations: Vec<TriggerEditOperation>,
}

pub(crate) fn read_input(file: Option<PathBuf>) -> Result<Value> {
    let raw = match file {
        Some(path) => storage::read(&path)?,
        None => storage::read_bounded(std::io::stdin().lock())?,
    };
    serde_json::from_slice(&raw).context("parsing trigger JSON input")
}

pub(crate) fn list(workspace: &Path) -> Result<Value> {
    let source = storage::manifest(workspace)?;
    let declarations = struo_frontend::load_triggers_toml_str("struo.toml", &source);
    let activation = registration::load(workspace)?;
    let (triggers, diagnostics) = match declarations {
        Ok(triggers) => (triggers, Vec::new()),
        Err(diagnostics) => (Vec::new(), diagnostics),
    };
    Ok(json!({
        "schema": "struo.triggers.v1",
        "triggers": triggers,
        "diagnostics": diagnostics,
        "activation": activation,
        "declaration_schema": schema::declaration(),
        "observation_schema": schema::observation(),
        "event": {"event": "vcs.ref_changed", "ref": "refs/heads/main", "revision": "<full commit object id>"},
        "next_calls": ["struo query validate-trigger", "struo edit triggers", "struo edit activate-triggers --revision <full commit object id>", "struo query trigger-plan <id> --file <observation.json>"],
    }))
}

pub(crate) fn validate(value: Value) -> Result<Value> {
    let input: ValidateInput = serde_json::from_value(value).context("parsing trigger draft")?;
    let diagnostics = struo_frontend::validate_triggers(&[input.trigger]);
    Ok(json!({"valid": diagnostics.is_empty(), "diagnostics": diagnostics}))
}

pub(crate) fn edit(workspace: &Path, value: Value) -> Result<Value> {
    let input: EditInput = serde_json::from_value(value).context("parsing trigger edit")?;
    let mut lock = storage::lock(workspace)?;
    let _guard = lock
        .try_write()
        .context("another trigger operation owns this workspace")?;
    let source = storage::manifest(workspace)?;
    let path = workspace.join("struo.toml");
    match struo_frontend::apply_trigger_operations(&source, &input.operations) {
        Ok(updated) => {
            if updated != source {
                storage::atomic_write(&path, updated.as_bytes())?;
            }
            Ok(json!({"applied": true, "changed": updated != source, "path": path}))
        }
        Err(diagnostics) => Ok(json!({"applied": false, "diagnostics": diagnostics})),
    }
}

pub(crate) async fn write(output: Output, value: &Value) -> Result<()> {
    let body = match output.format {
        Format::Human => format!("{}\n", serde_json::to_string_pretty(value)?),
        Format::Json | Format::Toon => crate::render::structured(output.format, value)?,
    };
    let mut stdout = tokio::io::stdout();
    stdout.write_all(body.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}
