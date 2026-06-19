use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use once_cas::{ActionResult, Digest};
use serde::{Deserialize, Serialize};

use crate::{Action, CacheState, Outcome};

pub(crate) const EVIDENCE_SCHEMA: &str = "once.evidence.v1";
pub(crate) const ACTION_RESULT_KIND: &str = "action_result";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceSubject {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}

impl EvidenceSubject {
    pub fn command(action_digest: Digest) -> Self {
        Self {
            kind: "command".to_string(),
            id: action_digest.to_string(),
            capability: None,
        }
    }

    pub fn target(target_id: impl Into<String>, capability: impl Into<String>) -> Self {
        Self {
            kind: "target".to_string(),
            id: target_id.into(),
            capability: Some(capability.into()),
        }
    }

    pub fn matches(&self, raw: &str) -> bool {
        self.id == raw
            || self
                .capability
                .as_ref()
                .is_some_and(|capability| format!("{}:{capability}", self.id) == raw)
    }

    pub fn display(&self) -> String {
        self.capability.as_ref().map_or_else(
            || self.id.clone(),
            |capability| format!("{}:{capability}", self.id),
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Passed,
    Failed,
}

impl EvidenceStatus {
    fn from_exit_code(exit_code: i32) -> Self {
        if exit_code == 0 {
            Self::Passed
        } else {
            Self::Failed
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_storage(raw: &str) -> Result<Self> {
        match raw {
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            _ => Err(anyhow!("unknown evidence status `{raw}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCacheState {
    Hit,
    Miss,
    Bypass,
}

impl EvidenceCacheState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Bypass => "bypass",
        }
    }

    pub(crate) fn from_storage(raw: &str) -> Result<Self> {
        match raw {
            "hit" => Ok(Self::Hit),
            "miss" => Ok(Self::Miss),
            "bypass" => Ok(Self::Bypass),
            _ => Err(anyhow!("unknown evidence cache state `{raw}`")),
        }
    }
}

impl From<CacheState> for EvidenceCacheState {
    fn from(value: CacheState) -> Self {
        match value {
            CacheState::Hit => Self::Hit,
            CacheState::Miss => Self::Miss,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRecord {
    pub schema: String,
    pub id: String,
    pub kind: String,
    pub subject: EvidenceSubject,
    pub status: EvidenceStatus,
    pub action_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_digest: Option<Digest>,
    pub cache: EvidenceCacheState,
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<Digest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Digest>,
    pub created_at_unix_ms: i64,
}

impl EvidenceRecord {
    pub fn from_outcome(
        subject: EvidenceSubject,
        action: &Action,
        outcome: &Outcome,
    ) -> Result<Self> {
        Self::from_action_result(
            subject,
            outcome.action,
            action.input_digest(),
            EvidenceCacheState::from(outcome.cache),
            &outcome.result,
        )
    }

    pub fn from_action_result(
        subject: EvidenceSubject,
        action_digest: Digest,
        input_digest: Option<Digest>,
        cache: EvidenceCacheState,
        result: &ActionResult,
    ) -> Result<Self> {
        let created_at_unix_ms = unix_ms_now()?;
        let id = evidence_id(
            &subject,
            action_digest,
            cache,
            result.exit_code,
            created_at_unix_ms,
        )?;
        Ok(Self {
            schema: EVIDENCE_SCHEMA.to_string(),
            id,
            kind: ACTION_RESULT_KIND.to_string(),
            subject,
            status: EvidenceStatus::from_exit_code(result.exit_code),
            action_digest,
            input_digest,
            cache,
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            outputs: result.outputs.clone(),
            created_at_unix_ms,
        })
    }
}

fn evidence_id(
    subject: &EvidenceSubject,
    action_digest: Digest,
    cache: EvidenceCacheState,
    exit_code: i32,
    created_at_unix_ms: i64,
) -> Result<String> {
    let material = serde_json::to_vec(&(
        EVIDENCE_SCHEMA,
        ACTION_RESULT_KIND,
        subject,
        action_digest,
        cache,
        exit_code,
        created_at_unix_ms,
    ))
    .context("serializing evidence id material")?;
    Ok(Digest::of_bytes(&material).to_string())
}

fn unix_ms_now() -> Result<i64> {
    unix_ms(SystemTime::now())
}

fn unix_ms(time: SystemTime) -> Result<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis())
            .context("evidence timestamp does not fit SQLite integer"),
        Err(err) => {
            let millis = i64::try_from(err.duration().as_millis())
                .context("negative evidence timestamp does not fit SQLite integer")?;
            millis
                .checked_neg()
                .context("negative evidence timestamp overflow")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_subject_matches_id_and_capability_form() {
        let subject = EvidenceSubject::target("cli", "test");

        assert!(subject.matches("cli"));
        assert!(subject.matches("cli:test"));
        assert_eq!(subject.display(), "cli:test");
        assert!(!subject.matches("other"));
    }

    #[test]
    fn command_subject_uses_action_digest() {
        let digest = Digest::of_bytes(b"action");
        let subject = EvidenceSubject::command(digest);

        assert_eq!(subject.kind, "command");
        assert_eq!(subject.id, digest.to_string());
        assert_eq!(subject.capability, None);
        assert!(subject.matches(&digest.to_string()));
    }

    #[test]
    fn storage_conversions_accept_known_values() {
        assert_eq!(
            EvidenceStatus::from_storage("passed").unwrap(),
            EvidenceStatus::Passed
        );
        assert_eq!(
            EvidenceStatus::from_storage("failed").unwrap(),
            EvidenceStatus::Failed
        );
        assert_eq!(
            EvidenceCacheState::from_storage("hit").unwrap(),
            EvidenceCacheState::Hit
        );
        assert_eq!(
            EvidenceCacheState::from_storage("miss").unwrap(),
            EvidenceCacheState::Miss
        );
        assert_eq!(
            EvidenceCacheState::from_storage("bypass").unwrap(),
            EvidenceCacheState::Bypass
        );
    }

    #[test]
    fn storage_conversions_reject_unknown_values() {
        assert!(EvidenceStatus::from_storage("unknown").is_err());
        assert!(EvidenceCacheState::from_storage("skipped").is_err());
    }

    #[test]
    fn pre_epoch_times_are_negative_unix_milliseconds() {
        let before_epoch = UNIX_EPOCH
            .checked_sub(std::time::Duration::from_millis(42))
            .unwrap();

        assert_eq!(unix_ms(before_epoch).unwrap(), -42);
    }
}
