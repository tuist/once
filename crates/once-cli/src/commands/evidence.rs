use std::path::Path;

use once_cas::{ActionResult, Digest};
use once_core::{
    Action, EvidenceCacheState, EvidenceRecord, EvidenceStore, EvidenceSubject, Outcome,
};

pub async fn record_outcome(
    workspace: &Path,
    subject: EvidenceSubject,
    action: &Action,
    outcome: &Outcome,
) {
    let record = EvidenceRecord::from_outcome(subject, action, outcome);
    append_record(workspace, &record).await;
}

pub async fn record_action_result(
    workspace: &Path,
    subject: EvidenceSubject,
    action_digest: Digest,
    input_digest: Option<Digest>,
    cache: EvidenceCacheState,
    result: &ActionResult,
) {
    let record =
        EvidenceRecord::from_action_result(subject, action_digest, input_digest, cache, result);
    append_record(workspace, &record).await;
}

async fn append_record(workspace: &Path, record: &EvidenceRecord) {
    let store = EvidenceStore::open_workspace(workspace);
    if let Err(err) = store.append(record).await {
        tracing::warn!(
            error = %err,
            path = %store.path().display(),
            evidence = %record.id,
            "failed to record evidence"
        );
    }
}
