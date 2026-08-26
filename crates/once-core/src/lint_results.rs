use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LINT_RESULTS_SCHEMA: &str = "once.lint_results.v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum LintSeverity {
    None,
    Note,
    Warning,
    Error,
}

impl LintSeverity {
    #[must_use]
    pub fn from_sarif(level: Option<&str>) -> Self {
        match level {
            Some("error") => Self::Error,
            Some("note") => Self::Note,
            Some("none") => Self::None,
            _ => Self::Warning,
        }
    }
}

impl FromStr for LintSeverity {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "none" => Ok(Self::None),
            "note" => Ok(Self::Note),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "expected `none`, `note`, `warning`, or `error`, got `{raw}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LintLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LintFinding {
    pub analyzer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    pub severity: LintSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<LintLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LintSummary {
    pub total: usize,
    pub errors: usize,
    pub warnings: usize,
    pub notes: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LintResults {
    pub schema: String,
    pub target: String,
    pub status: String,
    pub complete: bool,
    pub summary: LintSummary,
    pub findings: Vec<LintFinding>,
    pub artifacts: LintArtifacts,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LintArtifacts {
    pub portable_report: String,
}

impl LintResults {
    pub fn from_sarif(
        target: impl Into<String>,
        sarif_path: impl Into<String>,
        document: &Value,
        workspace: &Path,
    ) -> Result<Self> {
        let runs = document
            .get("runs")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("lint report does not contain a `runs` array"))?;
        let mut findings = Vec::new();
        for run in runs {
            let analyzer = run
                .pointer("/tool/driver/name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let rules = run
                .pointer("/tool/driver/rules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for result in run
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let rule_id = result
                    .get("ruleId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let help_uri = rule_id.as_deref().and_then(|wanted| {
                    rules.iter().find_map(|rule| {
                        (rule.get("id").and_then(Value::as_str) == Some(wanted))
                            .then(|| rule.get("helpUri").and_then(Value::as_str))
                            .flatten()
                            .map(str::to_string)
                    })
                });
                let location = result
                    .pointer("/locations/0/physicalLocation")
                    .map(|value| lint_location(value, workspace));
                let fingerprint = result
                    .get("partialFingerprints")
                    .and_then(Value::as_object)
                    .and_then(|values| values.values().find_map(Value::as_str))
                    .map(str::to_string);
                findings.push(LintFinding {
                    analyzer: analyzer.clone(),
                    rule_id,
                    severity: LintSeverity::from_sarif(result.get("level").and_then(Value::as_str)),
                    message: result
                        .pointer("/message/text")
                        .and_then(Value::as_str)
                        .or_else(|| result.pointer("/message/markdown").and_then(Value::as_str))
                        .unwrap_or("lint finding")
                        .to_string(),
                    location,
                    help_uri,
                    fingerprint,
                });
            }
        }
        findings.sort_by(|left, right| {
            let left_location = left.location.as_ref();
            let right_location = right.location.as_ref();
            (
                left_location.and_then(|location| location.path.as_deref()),
                left_location.and_then(|location| location.line),
                left.rule_id.as_deref(),
                left.message.as_str(),
            )
                .cmp(&(
                    right_location.and_then(|location| location.path.as_deref()),
                    right_location.and_then(|location| location.line),
                    right.rule_id.as_deref(),
                    right.message.as_str(),
                ))
        });
        let mut summary = LintSummary {
            total: findings.len(),
            ..LintSummary::default()
        };
        for finding in &findings {
            match finding.severity {
                LintSeverity::Error => summary.errors += 1,
                LintSeverity::Warning => summary.warnings += 1,
                LintSeverity::Note => summary.notes += 1,
                LintSeverity::None => {}
            }
        }
        Ok(Self {
            schema: LINT_RESULTS_SCHEMA.to_string(),
            target: target.into(),
            status: "completed".to_string(),
            complete: true,
            summary,
            findings,
            artifacts: LintArtifacts {
                portable_report: sarif_path.into(),
            },
        })
    }

    #[must_use]
    pub fn fails_at(&self, threshold: LintSeverity) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity >= threshold)
    }
}

fn lint_location(value: &Value, workspace: &Path) -> LintLocation {
    LintLocation {
        path: value
            .pointer("/artifactLocation/uri")
            .and_then(Value::as_str)
            .map(|uri| normalize_path(uri, workspace)),
        line: value.pointer("/region/startLine").and_then(Value::as_u64),
        column: value.pointer("/region/startColumn").and_then(Value::as_u64),
        end_line: value.pointer("/region/endLine").and_then(Value::as_u64),
        end_column: value.pointer("/region/endColumn").and_then(Value::as_u64),
    }
}

fn normalize_path(uri: &str, workspace: &Path) -> String {
    let path = reqwest::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .unwrap_or_else(|| PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri)));
    if path.is_absolute() {
        path.strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        path.to_string_lossy()
            .trim_start_matches("./")
            .replace('\\', "/")
    }
}

pub fn read_sarif_results(
    target: &str,
    report_path: &str,
    workspace: &Path,
) -> Result<LintResults> {
    let absolute = workspace.join(report_path);
    let bytes = std::fs::read(&absolute)
        .with_context(|| format!("reading lint report `{}`", absolute.display()))?;
    let document = serde_json::from_slice(&bytes).context("decoding lint report as JSON")?;
    LintResults::from_sarif(target, report_path, &document, workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_sarif_into_stable_findings() {
        let document = json!({
            "version": "2.1.0",
            "runs": [{
                "tool": {"driver": {
                    "name": "example",
                    "rules": [{"id": "E1", "helpUri": "https://example.com/E1"}]
                }},
                "results": [{
                    "ruleId": "E1",
                    "level": "error",
                    "message": {"text": "broken"},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": "src/main.rs"},
                        "region": {"startLine": 3, "startColumn": 5}
                    }}]
                }]
            }]
        });
        let results =
            LintResults::from_sarif("//app:lint", "out/report.sarif", &document, Path::new("."))
                .unwrap();
        assert_eq!(results.schema, LINT_RESULTS_SCHEMA);
        assert_eq!(results.summary.errors, 1);
        assert_eq!(results.findings[0].rule_id.as_deref(), Some("E1"));
        assert_eq!(
            results.findings[0]
                .location
                .as_ref()
                .and_then(|location| location.path.as_deref()),
            Some("src/main.rs")
        );
        assert!(results.fails_at(LintSeverity::Warning));
    }

    #[test]
    fn decodes_file_uri_paths_before_making_them_relative() {
        let workspace = tempfile::tempdir().unwrap();
        let file = workspace.path().join("src dir/main.rs");
        let uri = reqwest::Url::from_file_path(&file).unwrap();

        assert_eq!(
            normalize_path(uri.as_str(), workspace.path()),
            "src dir/main.rs"
        );
    }
}
