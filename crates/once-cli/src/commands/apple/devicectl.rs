use anyhow::Result;
use serde::Deserialize;
use std::process::Command;

use super::destination::{
    unavailable_tool_destination, AppleDestination, AppleDestinationKind, AppleDestinationSelector,
    AppleDestinationSupport,
};

pub fn list() -> Result<Vec<AppleDestination>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("devices.json");
    let output = Command::new("xcrun")
        .args(["devicectl", "list", "devices", "--json-output"])
        .arg(&path)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let raw = std::fs::read(&path)?;
            parse_devices(&raw)
        }
        Ok(output) => Ok(vec![unavailable_tool_destination(
            AppleDestinationKind::Device,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )]),
        Err(err) => Ok(vec![unavailable_tool_destination(
            AppleDestinationKind::Device,
            format!("xcrun devicectl unavailable: {err}"),
        )]),
    }
}

pub fn parse_devices(raw: &[u8]) -> Result<Vec<AppleDestination>> {
    let payload: DeviceCtlPayload = serde_json::from_slice(raw)?;
    Ok(payload
        .result
        .devices
        .into_iter()
        .map(|device| {
            let platform = device
                .platform
                .as_deref()
                .map(normalize_platform)
                .unwrap_or_else(|| "ios".to_string());
            AppleDestination {
                selector: AppleDestinationSelector {
                    kind: AppleDestinationKind::Device,
                    id: device.identifier,
                },
                display_name: device.name,
                platform,
                runtime: None,
                os_version: device.os_version,
                available: device.available.unwrap_or(true),
                support: AppleDestinationSupport {
                    supported: true,
                    reason: None,
                },
            }
        })
        .collect())
}

fn normalize_platform(platform: &str) -> String {
    if platform.eq_ignore_ascii_case("ios") || platform.contains("iPhone") {
        "ios".to_string()
    } else if platform.eq_ignore_ascii_case("tvos") {
        "tvos".to_string()
    } else {
        platform.to_ascii_lowercase()
    }
}

#[derive(Deserialize)]
struct DeviceCtlPayload {
    result: DeviceCtlResult,
}

#[derive(Deserialize)]
struct DeviceCtlResult {
    #[serde(default)]
    devices: Vec<DeviceCtlDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCtlDevice {
    #[serde(alias = "identifier", alias = "udid")]
    identifier: String,
    name: Option<String>,
    platform: Option<String>,
    os_version: Option<String>,
    available: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devices() {
        let raw = br#"{"result":{"devices":[{"identifier":"DEV-1","name":"iPhone","platform":"iOS","osVersion":"18.0","available":true}]}}"#;

        let destinations = parse_devices(raw).unwrap();

        assert_eq!(destinations.len(), 1);
        assert_eq!(destinations[0].selector.id, "DEV-1");
        assert_eq!(destinations[0].platform, "ios");
        assert!(destinations[0].support.supported);
    }
}
