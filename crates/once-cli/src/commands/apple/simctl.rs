use anyhow::Result;
use serde::Deserialize;
use std::process::Command;

use super::destination::{
    unavailable_tool_destination, AppleDestination, AppleDestinationKind, AppleDestinationSelector,
    AppleDestinationSupport,
};

pub fn list() -> Result<Vec<AppleDestination>> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "devices", "--json"])
        .output();
    match output {
        Ok(output) if output.status.success() => parse_devices(&output.stdout),
        Ok(output) => Ok(vec![unavailable_tool_destination(
            AppleDestinationKind::Simulator,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )]),
        Err(err) => Ok(vec![unavailable_tool_destination(
            AppleDestinationKind::Simulator,
            format!("xcrun simctl unavailable: {err}"),
        )]),
    }
}

pub fn parse_devices(raw: &[u8]) -> Result<Vec<AppleDestination>> {
    let payload: SimctlPayload = serde_json::from_slice(raw)?;
    let mut destinations = Vec::new();
    for (runtime, devices) in payload.devices {
        let platform = runtime_platform(&runtime);
        let os_version = runtime_version(&runtime);
        for device in devices {
            destinations.push(AppleDestination {
                selector: AppleDestinationSelector {
                    kind: AppleDestinationKind::Simulator,
                    id: device.udid,
                },
                display_name: Some(device.name),
                platform: platform.clone(),
                runtime: Some(runtime.clone()),
                os_version: os_version.clone(),
                available: device.is_available.unwrap_or(true),
                support: AppleDestinationSupport {
                    supported: true,
                    reason: None,
                },
            });
        }
    }
    Ok(destinations)
}

fn runtime_platform(runtime: &str) -> String {
    if runtime.contains("iOS") {
        "ios"
    } else if runtime.contains("tvOS") {
        "tvos"
    } else if runtime.contains("watchOS") {
        "watchos"
    } else if runtime.contains("xrOS") || runtime.contains("visionOS") {
        "visionos"
    } else {
        "unknown"
    }
    .to_string()
}

fn runtime_version(runtime: &str) -> Option<String> {
    let runtime_name = runtime.rsplit('.').next().unwrap_or(runtime);
    runtime_name
        .split_once('-')
        .map(|(_, version)| version.replace('-', "."))
}

#[derive(Deserialize)]
struct SimctlPayload {
    devices: std::collections::BTreeMap<String, Vec<SimctlDevice>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimctlDevice {
    name: String,
    udid: String,
    is_available: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ios_simulators() {
        let raw = br#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{"name":"iPhone 16","udid":"SIM-1","isAvailable":true}]}}"#;

        let destinations = parse_devices(raw).unwrap();

        assert_eq!(destinations.len(), 1);
        assert_eq!(destinations[0].selector.id, "SIM-1");
        assert_eq!(destinations[0].platform, "ios");
        assert_eq!(destinations[0].os_version.as_deref(), Some("18.0"));
        assert!(destinations[0].support.supported);
    }
}
