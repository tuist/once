mod path_fingerprint;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use once_cas::Digest;
use once_core::SandboxMode;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityRunRecord;
use crate::commands::change_tracker::{ChangePosition, ChangeSnapshot};

use self::path_fingerprint::PathFingerprint;

const SCHEMA: &str = "once.build-receipt.v5";

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct BuildReceipt {
    schema: String,
    target: String,
    sandbox: String,
    environment: BTreeMap<String, Option<String>>,
    host_paths: BTreeMap<String, Option<PathFingerprint>>,
    source_digests: BTreeMap<String, Digest>,
    executable_fingerprint: PathFingerprint,
    position: ChangePosition,
    record: CapabilityRunRecord,
}

pub(super) struct Observations<'a> {
    pub environment: &'a BTreeMap<String, Option<String>>,
    pub host_paths: &'a BTreeSet<PathBuf>,
    pub source_digests: &'a BTreeMap<String, Digest>,
}

pub(super) async fn read(
    workspace: &Path,
    target: &str,
    sandbox: SandboxMode,
) -> Option<BuildReceipt> {
    let path = receipt_path(workspace, target, sandbox);
    let raw = tokio::fs::read(&path).await.ok()?;
    serde_json::from_slice::<BuildReceipt>(&raw).ok()
}

pub(super) fn position(receipt: &BuildReceipt) -> &ChangePosition {
    &receipt.position
}

pub(super) async fn load(
    workspace: &Path,
    target: &str,
    sandbox: SandboxMode,
    snapshot: Option<&ChangeSnapshot>,
    receipt: Option<BuildReceipt>,
) -> Option<CapabilityRunRecord> {
    let snapshot = snapshot?;
    let mut receipt = receipt?;
    let changed_environment = changed_environment(&receipt.environment);
    let changed_host_paths = path_fingerprint::changed(&receipt.host_paths);
    let miss = if receipt.schema != SCHEMA {
        Some("schema")
    } else if receipt.target != target {
        Some("target")
    } else if receipt.sandbox != sandbox_key(sandbox) {
        Some("sandbox")
    } else if receipt.position.instance_id != snapshot.position.instance_id {
        Some("filesystem tracker")
    } else if receipt.position.source_generation != snapshot.position.source_generation
        && !source_changes_match(
            workspace,
            &receipt.source_digests,
            snapshot.source_changes.as_deref(),
        )
    {
        Some("source change")
    } else if receipt.position.output_generation != snapshot.position.output_generation {
        Some("output change")
    } else if !changed_environment.is_empty() {
        Some("environment")
    } else if !changed_host_paths.is_empty() {
        Some("host path")
    } else if receipt.executable_fingerprint != executable_fingerprint()? {
        Some("executable")
    } else if !receipt
        .record
        .outputs
        .iter()
        .all(|output| workspace.join(output).exists())
    {
        Some("output")
    } else {
        None
    };
    if let Some(reason) = miss {
        tracing::debug!(
            target,
            reason,
            changed_environment = ?changed_environment,
            changed_host_paths = ?changed_host_paths,
            "build receipt requires validation"
        );
        return None;
    }
    if receipt.position != snapshot.position {
        receipt.position = snapshot.position.clone();
        receipt.source_digests.clear();
        if let Err(error) = write_receipt(&receipt_path(workspace, target, sandbox), &receipt).await
        {
            tracing::debug!(%error, target, "failed to advance reconciled build receipt");
        }
    }
    receipt.record.cache = "hit".to_string();
    tracing::debug!(
        target,
        source_generation = snapshot.position.source_generation,
        output_generation = snapshot.position.output_generation,
        "reused unchanged build receipt"
    );
    Some(receipt.record)
}

pub(super) async fn store(
    workspace: &Path,
    target: &str,
    sandbox: SandboxMode,
    snapshot: &ChangeSnapshot,
    observations: Observations<'_>,
    record: &CapabilityRunRecord,
) {
    let Some(executable_fingerprint) = executable_fingerprint() else {
        return;
    };
    let receipt = BuildReceipt {
        schema: SCHEMA.to_string(),
        target: target.to_string(),
        sandbox: sandbox_key(sandbox),
        environment: observations.environment.clone(),
        host_paths: path_fingerprint::capture(observations.host_paths),
        source_digests: observations.source_digests.clone(),
        executable_fingerprint,
        position: snapshot.position.clone(),
        record: record.clone(),
    };
    if let Err(error) = write_receipt(&receipt_path(workspace, target, sandbox), &receipt).await {
        tracing::debug!(%error, target, "failed to persist build receipt");
    }
}

/// Remove any stored receipt for a target so a later invocation cannot reuse
/// it. Used when the completed build is uncacheable and must run every time.
pub(super) async fn clear(workspace: &Path, target: &str, sandbox: SandboxMode) {
    let path = receipt_path(workspace, target, sandbox);
    if let Err(error) = tokio::fs::remove_file(&path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(%error, target, "failed to clear build receipt");
        }
    }
}

async fn write_receipt(path: &Path, receipt: &BuildReceipt) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .expect("build receipt path always has a parent");
    tokio::fs::create_dir_all(parent).await?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    tokio::fs::write(&temporary, serde_json::to_vec(receipt)?).await?;
    tokio::fs::rename(temporary, path).await?;
    Ok(())
}

fn receipt_path(workspace: &Path, target: &str, sandbox: SandboxMode) -> PathBuf {
    let key = Digest::of_bytes(format!("{target}\0{}", sandbox_key(sandbox)).as_bytes());
    workspace
        .join(".once")
        .join("build-receipts")
        .join(format!("{key}.json"))
}

fn sandbox_key(sandbox: SandboxMode) -> String {
    format!("{sandbox:?}")
}

fn changed_environment(expected: &BTreeMap<String, Option<String>>) -> Vec<String> {
    expected
        .iter()
        .filter(|(name, expected)| std::env::var(name).ok() != **expected)
        .map(|(name, _)| name.clone())
        .collect()
}

fn source_changes_match(
    workspace: &Path,
    expected: &BTreeMap<String, Digest>,
    changes: Option<&[String]>,
) -> bool {
    let Some(changes) = changes else {
        return false;
    };
    changes.iter().all(|path| {
        expected.get(path).is_some_and(|expected| {
            once_core::digest_source_path(workspace, path).is_ok_and(|actual| actual == *expected)
        })
    })
}

fn executable_fingerprint() -> Option<PathFingerprint> {
    let executable = std::env::current_exe().ok()?;
    path_fingerprint::fingerprint(&executable)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use once_cas::ActionResult;
    use once_core::EvidenceCacheState;
    use tempfile::TempDir;

    use super::*;

    fn record() -> CapabilityRunRecord {
        CapabilityRunRecord {
            target: "app".to_string(),
            kind: "test".to_string(),
            capability: "build".to_string(),
            status: "completed".to_string(),
            action_digest: "abc".to_string(),
            cache: "miss".to_string(),
            output_groups: vec!["binary".to_string()],
            required_outputs: Vec::new(),
            outputs: vec![".once/out/app/app".to_string()],
            test_results: None,
            input_digest: None,
            cache_state: EvidenceCacheState::Miss,
            result: ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs: BTreeMap::new(),
            },
        }
    }

    fn snapshot(source_generation: u64, output_generation: u64) -> ChangeSnapshot {
        ChangeSnapshot {
            position: ChangePosition {
                instance_id: "tracker".to_string(),
                source_generation,
                output_generation,
            },
            source_changes: Some(Vec::new()),
            output_changes: Some(Vec::new()),
        }
    }

    #[tokio::test]
    async fn receipt_requires_the_same_tracker_snapshot_and_outputs() {
        let temporary = TempDir::new().unwrap();
        let output = temporary.path().join(".once/out/app/app");
        tokio::fs::create_dir_all(output.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&output, b"binary").await.unwrap();
        let snapshot = snapshot(3, 5);
        let environment = BTreeMap::new();
        let no_host_paths = BTreeSet::new();
        let source_digests = BTreeMap::new();
        store(
            temporary.path(),
            "app",
            SandboxMode::Off,
            &snapshot,
            Observations {
                environment: &environment,
                host_paths: &no_host_paths,
                source_digests: &source_digests,
            },
            &record(),
        )
        .await;

        let receipt = read(temporary.path(), "app", SandboxMode::Off).await;
        let loaded = load(
            temporary.path(),
            "app",
            SandboxMode::Off,
            Some(&snapshot),
            receipt,
        )
        .await
        .unwrap();
        assert_eq!(loaded.cache, "hit");

        let changed = ChangeSnapshot {
            position: ChangePosition {
                source_generation: 4,
                ..snapshot.position.clone()
            },
            source_changes: None,
            ..snapshot.clone()
        };
        let receipt = read(temporary.path(), "app", SandboxMode::Off).await;
        assert!(load(
            temporary.path(),
            "app",
            SandboxMode::Off,
            Some(&changed),
            receipt,
        )
        .await
        .is_none());

        let host_tool = temporary.path().join("tool");
        tokio::fs::write(&host_tool, b"one").await.unwrap();
        let host_paths = BTreeSet::from([host_tool.clone()]);
        store(
            temporary.path(),
            "app",
            SandboxMode::Off,
            &snapshot,
            Observations {
                environment: &environment,
                host_paths: &host_paths,
                source_digests: &source_digests,
            },
            &record(),
        )
        .await;
        let receipt = read(temporary.path(), "app", SandboxMode::Off).await;
        assert!(load(
            temporary.path(),
            "app",
            SandboxMode::Off,
            Some(&snapshot),
            receipt,
        )
        .await
        .is_some());

        tokio::fs::write(host_tool, b"two").await.unwrap();
        let receipt = read(temporary.path(), "app", SandboxMode::Off).await;
        assert!(load(
            temporary.path(),
            "app",
            SandboxMode::Off,
            Some(&snapshot),
            receipt,
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn receipt_reconciles_an_identical_observed_source_rewrite() {
        let temporary = TempDir::new().unwrap();
        let source = temporary.path().join("src/main.rs");
        let output = temporary.path().join(".once/out/app/app");
        tokio::fs::create_dir_all(source.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::create_dir_all(output.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&source, b"source").await.unwrap();
        tokio::fs::write(&output, b"binary").await.unwrap();
        let initial = snapshot(3, 5);
        let environment = BTreeMap::new();
        let host_paths = BTreeSet::new();
        let source_digests =
            BTreeMap::from([("src/main.rs".to_string(), Digest::of_bytes(b"source"))]);
        store(
            temporary.path(),
            "app",
            SandboxMode::Off,
            &initial,
            Observations {
                environment: &environment,
                host_paths: &host_paths,
                source_digests: &source_digests,
            },
            &record(),
        )
        .await;

        let changed = ChangeSnapshot {
            position: ChangePosition {
                source_generation: 4,
                ..initial.position.clone()
            },
            source_changes: Some(vec!["src/main.rs".to_string()]),
            ..initial.clone()
        };
        let receipt = read(temporary.path(), "app", SandboxMode::Off).await;
        assert!(load(
            temporary.path(),
            "app",
            SandboxMode::Off,
            Some(&changed),
            receipt,
        )
        .await
        .is_some());

        tokio::fs::write(source, b"changed").await.unwrap();
        let changed_again = ChangeSnapshot {
            position: ChangePosition {
                source_generation: 5,
                ..initial.position
            },
            source_changes: Some(vec!["src/main.rs".to_string()]),
            ..changed
        };
        let receipt = read(temporary.path(), "app", SandboxMode::Off).await;
        assert!(load(
            temporary.path(),
            "app",
            SandboxMode::Off,
            Some(&changed_again),
            receipt,
        )
        .await
        .is_none());
    }
}
