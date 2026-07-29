use serde::{Deserialize, Serialize};

use super::{ChangePosition, ChangeSnapshot};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Request {
    pub command: String,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub since: Option<ChangePosition>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ChangeSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub(super) fn success(snapshot: ChangeSnapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            error: None,
        }
    }

    pub(super) fn error(error: impl Into<String>) -> Self {
        Self {
            snapshot: None,
            error: Some(error.into()),
        }
    }
}
