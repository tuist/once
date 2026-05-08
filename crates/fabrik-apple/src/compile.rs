use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fabrik_cas::Digest;
use fabrik_core::{Action, PlanNode, ResourceRequest, WorkspacePath};
use fabrik_frontend::Target;

use crate::artifact::app_bundle_path;

#[derive(Debug, Clone)]
pub struct AppleAction {
    pub action: Action,
    pub output: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AppleError {
    #[error("target {label} has unsupported kind `{kind}`")]
    UnsupportedKind { label: String, kind: String },
    #[error("target {label} is missing required attr `{attr}`")]
    MissingAttr { label: String, attr: String },
    #[error("target {label} has no Swift sources")]
    NoSources { label: String },
    #[error("target {label}: invalid path `{path}`: {source}")]
    InvalidPath {
        label: String,
        path: String,
        #[source]
        source: fabrik_core::WorkspacePathError,
    },
    #[error("failed to read source `{path}` for target {label}: {source}")]
    ReadSource {
        label: String,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone, Copy)]
enum Mode {
    Build,
    Launch,
}

pub fn compile_ios_app(target: &Target, workspace_root: &Path) -> Result<PlanNode, AppleError> {
    let action = build_ios_app_action(target, workspace_root)?;
    Ok(PlanNode {
        label: target.label(),
        action: action.action,
        deps: Vec::new(),
    })
}

pub fn launch_ios_app(target: &Target, _workspace_root: &Path) -> Result<AppleAction, AppleError> {
    if target.kind != "apple_ios_app" {
        return Err(AppleError::UnsupportedKind {
            label: target.label(),
            kind: target.kind.clone(),
        });
    }

    let bundle_id = required_attr(target, "bundle_id")?;
    let app_dir = app_bundle_path(&target.package, &target.name);
    let simulator = target
        .attrs
        .get("simulator")
        .cloned()
        .unwrap_or_else(|| "booted".to_string());

    Ok(AppleAction {
        action: Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                launch_script(&app_dir, &bundle_id, &simulator),
            ],
            env: tool_env(Mode::Launch),
            cwd: None,
            input_digest: Some(uncached_launch_digest(target, &bundle_id)),
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(600_000),
        },
        output: app_dir,
    })
}

fn build_ios_app_action(target: &Target, workspace_root: &Path) -> Result<AppleAction, AppleError> {
    if target.kind != "apple_ios_app" {
        return Err(AppleError::UnsupportedKind {
            label: target.label(),
            kind: target.kind.clone(),
        });
    }
    if target.srcs.is_empty() {
        return Err(AppleError::NoSources {
            label: target.label(),
        });
    }

    let bundle_id = required_attr(target, "bundle_id")?;
    let executable_name = target
        .attrs
        .get("executable_name")
        .cloned()
        .unwrap_or_else(|| target.name.clone());
    let minimum_os = target
        .attrs
        .get("minimum_os")
        .cloned()
        .unwrap_or_else(|| "17.0".to_string());
    let app_dir = app_bundle_path(&target.package, &target.name);
    let source_paths = source_paths(target)?;
    let source_args = source_paths
        .iter()
        .map(|s| sh_quote(s))
        .collect::<Vec<_>>()
        .join(" ");

    let build_script = build_script(
        target,
        &bundle_id,
        &executable_name,
        &minimum_os,
        &app_dir,
        &source_args,
    );
    let input_digest = build_input_digest(target, workspace_root, &bundle_id, &minimum_os)?;
    let output =
        WorkspacePath::try_from(app_dir.as_str()).map_err(|source| AppleError::InvalidPath {
            label: target.label(),
            path: app_dir.clone(),
            source,
        })?;

    Ok(AppleAction {
        action: Action::RunCommand {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), build_script],
            env: tool_env(Mode::Build),
            cwd: None,
            input_digest: Some(input_digest),
            outputs: vec![output],
            resources: ResourceRequest::new(2, 0),
            timeout_ms: Some(300_000),
        },
        output: app_dir,
    })
}

fn build_script(
    target: &Target,
    bundle_id: &str,
    executable_name: &str,
    minimum_os: &str,
    app_dir: &str,
    source_args: &str,
) -> String {
    let plist = info_plist(target, bundle_id, executable_name, minimum_os);
    format!(
        r#"set -eu
SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"
ARCH="$(uname -m)"
case "$ARCH" in
  arm64) FABRIK_TARGET="arm64-apple-ios{minimum_os}-simulator" ;;
  x86_64) FABRIK_TARGET="x86_64-apple-ios{minimum_os}-simulator" ;;
  *) echo "unsupported iOS simulator architecture: $ARCH" >&2; exit 2 ;;
esac
APP_DIR={app_dir}
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR"
cat > "$APP_DIR/Info.plist" <<'FABRIK_PLIST'
{plist}
FABRIK_PLIST
SDKROOT="$SDK" xcrun --sdk iphonesimulator swiftc -sdk "$SDK" -target "$FABRIK_TARGET" -parse-as-library -emit-executable -o "$APP_DIR/{executable_name}" {source_args}
codesign --force --sign - "$APP_DIR" >/dev/null
"#,
        minimum_os = sh_double(minimum_os),
        app_dir = sh_quote(app_dir),
        plist = plist,
        executable_name = sh_single_segment(executable_name),
        source_args = source_args,
    )
}

fn launch_script(app_dir: &str, bundle_id: &str, simulator: &str) -> String {
    format!(
        r#"DEVICE="${{FABRIK_IOS_SIMULATOR:-{simulator}}}"
if [ "$DEVICE" != "booted" ]; then
  xcrun simctl boot "$DEVICE" >/dev/null 2>&1 || true
  xcrun simctl bootstatus "$DEVICE" -b
fi
xcrun simctl install "$DEVICE" {app_dir}
xcrun simctl launch "$DEVICE" {bundle_id}
"#,
        simulator = sh_double(simulator),
        app_dir = sh_quote(app_dir),
        bundle_id = sh_quote(bundle_id),
    )
}

fn info_plist(target: &Target, bundle_id: &str, executable_name: &str, minimum_os: &str) -> String {
    let display_name = xml_escape(&target.name);
    let bundle_id = xml_escape(bundle_id);
    let executable_name = xml_escape(executable_name);
    let minimum_os = xml_escape(minimum_os);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>{executable_name}</string>
  <key>CFBundleIdentifier</key>
  <string>{bundle_id}</string>
  <key>CFBundleName</key>
  <string>{display_name}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>MinimumOSVersion</key>
  <string>{minimum_os}</string>
  <key>UILaunchScreen</key>
  <dict/>
</dict>
</plist>
"#
    )
}

fn required_attr(target: &Target, attr: &str) -> Result<String, AppleError> {
    target
        .attrs
        .get(attr)
        .cloned()
        .ok_or_else(|| AppleError::MissingAttr {
            label: target.label(),
            attr: attr.to_string(),
        })
}

fn source_paths(target: &Target) -> Result<Vec<String>, AppleError> {
    target
        .srcs
        .iter()
        .map(|src| {
            let rel = if target.package.is_empty() {
                src.clone()
            } else {
                format!("{}/{src}", target.package)
            };
            WorkspacePath::try_from(rel.as_str())
                .map(|p| p.as_str().to_string())
                .map_err(|source| AppleError::InvalidPath {
                    label: target.label(),
                    path: rel,
                    source,
                })
        })
        .collect()
}

fn build_input_digest(
    target: &Target,
    workspace_root: &Path,
    bundle_id: &str,
    minimum_os: &str,
) -> Result<Digest, AppleError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.apple.ios_app.input.v1\0");
    buf.extend_from_slice(bundle_id.as_bytes());
    buf.push(0);
    buf.extend_from_slice(minimum_os.as_bytes());
    buf.push(0);

    let mut srcs = source_paths(target)?;
    srcs.sort();
    for src in srcs {
        let abs = workspace_root.join(&src);
        let bytes = std::fs::read(&abs).map_err(|source| AppleError::ReadSource {
            label: target.label(),
            path: src.clone(),
            source,
        })?;
        let digest = Digest::of_bytes(&bytes);
        buf.extend_from_slice(src.as_bytes());
        buf.push(0);
        buf.extend_from_slice(digest.as_bytes());
        buf.push(0);
    }
    if let Ok(developer_dir) = std::env::var("DEVELOPER_DIR") {
        buf.extend_from_slice(b"developer_dir:");
        buf.extend_from_slice(developer_dir.as_bytes());
        buf.push(0);
    }
    Ok(Digest::of_bytes(&buf))
}

fn uncached_launch_digest(target: &Target, bundle_id: &str) -> Digest {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.apple.ios_app.launch.v1\0");
    buf.extend_from_slice(target.label().as_bytes());
    buf.push(0);
    buf.extend_from_slice(bundle_id.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&nonce.to_le_bytes());
    Digest::of_bytes(&buf)
}

fn tool_env(mode: Mode) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in ["PATH", "HOME", "DEVELOPER_DIR", "SDKROOT", "TOOLCHAINS"] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    if matches!(mode, Mode::Launch) {
        if let Ok(value) = std::env::var("FABRIK_IOS_SIMULATOR") {
            env.insert("FABRIK_IOS_SIMULATOR".into(), value);
        }
    }
    env
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn sh_double(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn sh_single_segment(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabrik_frontend::Target;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn app_target(srcs: &[&str]) -> Target {
        let mut attrs = BTreeMap::new();
        attrs.insert("bundle_id".to_string(), "dev.fabrik.demo".to_string());
        Target {
            package: "App".to_string(),
            kind: "apple_ios_app".to_string(),
            name: "Demo".to_string(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: Vec::new(),
            attrs,
        }
    }

    #[test]
    fn compiles_ios_app_to_swiftc_action() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("App/Sources")).unwrap();
        std::fs::write(tmp.path().join("App/Sources/App.swift"), "import SwiftUI").unwrap();
        let target = app_target(&["Sources/App.swift"]);
        let node = compile_ios_app(&target, tmp.path()).unwrap();
        let Action::RunCommand {
            argv,
            outputs,
            input_digest,
            ..
        } = node.action;
        assert_eq!(argv[0], "/bin/sh");
        assert!(argv[2].contains("swiftc"));
        assert!(argv[2].contains("xcrun --sdk iphonesimulator"));
        assert!(argv[2].contains("dev.fabrik.demo"));
        assert_eq!(outputs[0].as_str(), ".fabrik/out/App/Demo.app");
        assert!(input_digest.is_some());
    }

    #[test]
    fn missing_bundle_id_is_an_error() {
        let mut target = app_target(&["Sources/App.swift"]);
        target.attrs.clear();
        let err = compile_ios_app(&target, Path::new("/tmp")).unwrap_err();
        assert!(matches!(err, AppleError::MissingAttr { .. }));
    }
}
