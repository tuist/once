use super::runtime_descriptor::{RuntimeDescriptor, RuntimeInterface};

pub(super) fn is_apple_simulator_app(target: &fabrik_frontend::Target) -> bool {
    target.kind == "apple_ios_app"
        || (target.kind == "apple_simulator_app"
            && target
                .attrs
                .get("platform")
                .is_some_and(|platform| platform == "ios"))
}

pub(super) fn ios_simulator_descriptor(
    target_id: &str,
    target: &fabrik_frontend::Target,
) -> RuntimeDescriptor {
    let executable = target
        .attrs
        .get("executable_name")
        .cloned()
        .unwrap_or_else(|| target.name.clone());
    RuntimeDescriptor {
        kind: "ios_simulator".to_string(),
        subject: target_id.to_string(),
        session: None,
        rpc: None,
        capabilities: ["logs", "screenshot", "ui_tree", "ui_action"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        interfaces: ios_simulator_interfaces(&executable),
    }
}

fn ios_simulator_interfaces(executable: &str) -> Vec<RuntimeInterface> {
    vec![
        interface(
            "logs",
            "stream",
            [
                "xcrun",
                "simctl",
                "spawn",
                "booted",
                "log",
                "stream",
                "--style",
                "compact",
                "--predicate",
            ]
            .into_iter()
            .map(str::to_string)
            .chain([format!("process == \"{executable}\"")])
            .collect(),
            "Stream runtime logs for the launched simulator app",
        ),
        interface(
            "screenshot",
            "artifact",
            [
                "xcrun",
                "simctl",
                "io",
                "booted",
                "screenshot",
                "<output.png>",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            "Capture the visible simulator screen",
        ),
        interface(
            "ui_tree",
            "accessibility",
            ["axe", "describe-ui", "--udid", "booted"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            "Inspect the visible accessibility hierarchy",
        ),
        interface(
            "tap",
            "ui_action",
            [
                "axe",
                "tap",
                "--udid",
                "booted",
                "--label",
                "<accessibility-label>",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            "Interact with the UI through accessibility targeting",
        ),
    ]
}

fn interface(name: &str, kind: &str, argv: Vec<String>, description: &str) -> RuntimeInterface {
    RuntimeInterface {
        name: name.to_string(),
        kind: kind.to_string(),
        argv,
        description: Some(description.to_string()),
    }
}
