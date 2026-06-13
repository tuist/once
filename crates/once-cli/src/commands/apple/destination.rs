use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppleDestinationKind {
    Simulator,
    Device,
}

pub fn parse_destination_kind(value: &str) -> Result<AppleDestinationKind> {
    match value {
        "simulator" => Ok(AppleDestinationKind::Simulator),
        "device" => Ok(AppleDestinationKind::Device),
        other => anyhow::bail!("unsupported Apple destination kind `{other}`"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppleDestinationSelector {
    #[serde(rename = "destination_kind")]
    pub kind: AppleDestinationKind,
    #[serde(rename = "destination_id")]
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppleDestinationSupport {
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppleDestination {
    pub selector: AppleDestinationSelector,
    pub display_name: Option<String>,
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    pub available: bool,
    pub support: AppleDestinationSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppleDestinationValidation {
    pub valid: bool,
    pub target: String,
    pub destination: AppleDestinationSelector,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<AppleRunDiagnostic>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppleRunDiagnostic {
    pub code: String,
    pub severity: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<AppleDestinationSelector>,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<String>,
}

impl AppleRunDiagnostic {
    pub fn error(
        code: &str,
        phase: &str,
        target: Option<String>,
        destination: Option<AppleDestinationSelector>,
        message: impl Into<String>,
        repairs: Vec<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: "error".to_string(),
            phase: phase.to_string(),
            target,
            destination,
            message: message.into(),
            repairs,
        }
    }
}

pub fn validate(
    workspace: &Path,
    target_id: &str,
    selector: AppleDestinationSelector,
    destination: Option<AppleDestination>,
) -> Result<AppleDestinationValidation> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    if graph
        .iter()
        .find(|target| target.label.id == target_id)
        .is_none()
    {
        return Ok(missing_target_validation(target_id, selector));
    }
    Ok(validate_selector(target_id, selector, destination))
}

fn missing_target_validation(
    target_id: &str,
    selector: AppleDestinationSelector,
) -> AppleDestinationValidation {
    let repair = "Run `once query targets` and use a returned target id".to_string();
    let diagnostics = vec![AppleRunDiagnostic::error(
        "target_not_found",
        "validation",
        Some(target_id.to_string()),
        Some(selector.clone()),
        format!("no target matches `{target_id}`"),
        vec![repair.clone()],
    )];
    AppleDestinationValidation {
        valid: false,
        target: target_id.to_string(),
        destination: selector,
        diagnostics,
        repairs: vec![repair],
    }
}

pub fn validate_selector(
    target_id: &str,
    selector: AppleDestinationSelector,
    destination: Option<AppleDestination>,
) -> AppleDestinationValidation {
    let mut diagnostics = Vec::new();

    match destination {
        Some(destination) if !destination.available => diagnostics.push(AppleRunDiagnostic::error(
            "destination_unavailable",
            "validation",
            Some(target_id.to_string()),
            Some(selector.clone()),
            "selected Apple destination is not available",
            vec![
                "Choose an available destination from `once query apple-destinations`".to_string(),
            ],
        )),
        Some(destination) if !destination.support.supported => {
            diagnostics.push(AppleRunDiagnostic::error(
                "unsupported_destination",
                "validation",
                Some(target_id.to_string()),
                Some(selector.clone()),
                destination
                    .support
                    .reason
                    .unwrap_or_else(|| "selected Apple destination is not supported".to_string()),
                vec!["Choose an iOS simulator or tethered iOS device destination".to_string()],
            ))
        }
        None => diagnostics.push(AppleRunDiagnostic::error(
            "destination_unavailable",
            "validation",
            Some(target_id.to_string()),
            Some(selector.clone()),
            "selected Apple destination was not found",
            vec!["Run `once query apple-destinations` and use a returned selector".to_string()],
        )),
        Some(_) => {}
    }

    let repairs = diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.repairs.clone())
        .collect::<Vec<_>>();

    AppleDestinationValidation {
        valid: diagnostics.is_empty(),
        target: target_id.to_string(),
        destination: selector,
        diagnostics,
        repairs,
    }
}

pub fn unavailable_tool_destination(
    kind: AppleDestinationKind,
    message: String,
) -> AppleDestination {
    let id = match kind {
        AppleDestinationKind::Simulator => "simctl-unavailable",
        AppleDestinationKind::Device => "devicectl-unavailable",
    };
    AppleDestination {
        selector: AppleDestinationSelector {
            kind,
            id: id.to_string(),
        },
        display_name: None,
        platform: "ios".to_string(),
        runtime: None,
        os_version: None,
        available: false,
        support: AppleDestinationSupport {
            supported: false,
            reason: Some(message),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector(kind: AppleDestinationKind) -> AppleDestinationSelector {
        AppleDestinationSelector {
            kind,
            id: "D1".to_string(),
        }
    }

    #[test]
    fn simulator_validation_accepts_available_destination() {
        let validation = validate_selector(
            "apps/ios/App",
            selector(AppleDestinationKind::Simulator),
            Some(AppleDestination {
                selector: selector(AppleDestinationKind::Simulator),
                display_name: Some("iPhone".to_string()),
                platform: "ios".to_string(),
                runtime: Some("iOS 18.0".to_string()),
                os_version: Some("18.0".to_string()),
                available: true,
                support: AppleDestinationSupport {
                    supported: true,
                    reason: None,
                },
            }),
        );

        assert!(validation.valid);
    }

    #[test]
    fn validation_reports_missing_destination() {
        let validation =
            validate_selector("apps/ios/App", selector(AppleDestinationKind::Device), None);

        assert!(!validation.valid);
        assert_eq!(validation.diagnostics[0].code, "destination_unavailable");
    }

    #[test]
    fn validation_reports_missing_target() {
        let validation = missing_target_validation(
            "apps/ios/Missing",
            selector(AppleDestinationKind::Simulator),
        );

        assert!(!validation.valid);
        assert_eq!(validation.diagnostics[0].code, "target_not_found");
        assert_eq!(validation.target, "apps/ios/Missing");
    }
}
