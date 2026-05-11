//! `fabrik toolchain inspect` - print the project toolchain contract.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::Path;

use anyhow::{bail, Context, Result};
use fabrik_cas::Digest;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

const MISE_CONFIG: &str = "mise.toml";
const MISE_LOCK: &str = "mise.lock";

#[derive(Debug, Default, Deserialize)]
struct MiseConfig {
    #[serde(default)]
    tools: BTreeMap<String, toml::Value>,
    #[serde(default)]
    env: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Serialize)]
struct ToolchainView {
    source: &'static str,
    config: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lock: Option<String>,
    platform: PlatformView,
    tools: Vec<ToolView>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, toml::Value>,
    fingerprint: String,
}

#[derive(Debug, Serialize)]
struct PlatformView {
    os: &'static str,
    arch: &'static str,
    mise: String,
}

#[derive(Debug, Serialize)]
struct ToolView {
    name: String,
    request: toml::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    locked: Option<LockedToolView>,
}

#[derive(Debug, Serialize)]
struct LockedToolView {
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<LockedPlatformView>,
}

#[derive(Debug, Serialize)]
struct LockedPlatformView {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

pub async fn inspect(workspace: &Path, format: Format) -> Result<()> {
    let view = inspect_workspace(workspace)?;
    let body = match format {
        Format::Human => render_human(&view),
        Format::Json | Format::Toon => render::structured(format, &view)?,
    };

    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn inspect_workspace(workspace: &Path) -> Result<ToolchainView> {
    let config_path = workspace.join(MISE_CONFIG);
    let config_src = match std::fs::read_to_string(&config_path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            bail!("no {MISE_CONFIG} found at {}", config_path.display());
        }
        Err(source) => {
            return Err(source).with_context(|| format!("reading {}", config_path.display()));
        }
    };
    let config: MiseConfig = toml::from_str(&config_src)
        .with_context(|| format!("parsing {}", config_path.display()))?;

    let platform = current_platform();
    let lock_path = workspace.join(MISE_LOCK);
    let lock_src = match std::fs::read_to_string(&lock_path) {
        Ok(src) => Some(src),
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(source).with_context(|| format!("reading {}", lock_path.display()));
        }
    };
    let lock_root = lock_src
        .as_deref()
        .map(toml::from_str::<toml::Value>)
        .transpose()
        .with_context(|| format!("parsing {}", lock_path.display()))?;

    let tools = config
        .tools
        .iter()
        .map(|(name, request)| ToolView {
            name: name.clone(),
            request: request.clone(),
            locked: lock_root
                .as_ref()
                .and_then(|root| locked_tool(root, name, &platform.mise)),
        })
        .collect::<Vec<_>>();

    let fingerprint = fingerprint(&platform, &tools, &config.env)?;

    Ok(ToolchainView {
        source: "mise",
        config: MISE_CONFIG.to_string(),
        lock: lock_src.map(|_| MISE_LOCK.to_string()),
        platform,
        tools,
        env: config.env,
        fingerprint,
    })
}

fn locked_tool(root: &toml::Value, name: &str, platform_key: &str) -> Option<LockedToolView> {
    let records = root.get("tools")?.as_table()?.get(name)?.as_array()?;
    let record = records.first()?.as_table()?;

    let version = record.get("version")?.as_str()?.to_string();
    let backend = record
        .get("backend")
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned);
    let platform = record
        .get("platforms")
        .and_then(toml::Value::as_table)
        .and_then(|platforms| platforms.get(platform_key))
        .and_then(toml::Value::as_table)
        .map(|table| LockedPlatformView {
            key: platform_key.to_string(),
            checksum: table
                .get("checksum")
                .and_then(toml::Value::as_str)
                .map(ToOwned::to_owned),
            size: table.get("size").and_then(toml::Value::as_integer),
            url: table
                .get("url")
                .and_then(toml::Value::as_str)
                .map(ToOwned::to_owned),
        });

    Some(LockedToolView {
        version,
        backend,
        platform,
    })
}

fn fingerprint(
    platform: &PlatformView,
    tools: &[ToolView],
    env: &BTreeMap<String, toml::Value>,
) -> Result<String> {
    #[derive(Serialize)]
    struct Material<'a> {
        source: &'static str,
        platform: &'a PlatformView,
        tools: &'a [ToolView],
        env: &'a BTreeMap<String, toml::Value>,
    }

    let material = Material {
        source: "mise",
        platform,
        tools,
        env,
    };
    let bytes = serde_json::to_vec(&material).context("serializing toolchain fingerprint input")?;
    Ok(format!("blake3:{}", Digest::of_bytes(&bytes)))
}

fn current_platform() -> PlatformView {
    let arch = std::env::consts::ARCH;
    let mise_arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    PlatformView {
        os: std::env::consts::OS,
        arch,
        mise: format!("{}-{mise_arch}", std::env::consts::OS),
    }
}

fn render_human(view: &ToolchainView) -> String {
    let lock = view.lock.as_deref().unwrap_or("missing");
    let mut out = format!(
        "source: {}\nconfig: {}\nlock: {}\nplatform: {} ({}/{})\nfingerprint: {}\n",
        view.source,
        view.config,
        lock,
        view.platform.mise,
        view.platform.os,
        view.platform.arch,
        view.fingerprint
    );

    if view.tools.is_empty() {
        out.push_str("tools: none\n");
    } else {
        out.push_str("tools:\n");
        for tool in &view.tools {
            let request = format_toml_value(&tool.request);
            match &tool.locked {
                Some(locked) => {
                    writeln!(out, "  {} {} -> {}", tool.name, request, locked.version)
                        .expect("writing to string cannot fail");
                }
                None => {
                    writeln!(out, "  {} {}", tool.name, request)
                        .expect("writing to string cannot fail");
                }
            }
        }
    }

    if !view.env.is_empty() {
        out.push_str("env:\n");
        for (key, value) in &view.env {
            writeln!(out, "  {key} = {}", format_toml_value(value))
                .expect("writing to string cannot fail");
        }
    }

    out
}

fn format_toml_value(value: &toml::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn inspect_reads_mise_tools_and_env() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(MISE_CONFIG),
            r#"
[tools]
rust = "1.86"
shellspec = "latest"

[env]
RUST_BACKTRACE = "1"
"#,
        )
        .unwrap();

        let view = inspect_workspace(tmp.path()).unwrap();

        assert_eq!(view.source, "mise");
        assert_eq!(view.config, MISE_CONFIG);
        assert_eq!(view.lock, None);
        assert!(view.fingerprint.starts_with("blake3:"));
        assert_eq!(view.tools.len(), 2);
        assert_eq!(view.tools[0].name, "rust");
        assert_eq!(view.tools[0].request.as_str(), Some("1.86"));
        assert_eq!(
            view.env.get("RUST_BACKTRACE").and_then(toml::Value::as_str),
            Some("1")
        );
    }

    #[test]
    fn inspect_attaches_lockfile_versions_for_current_platform() {
        let tmp = TempDir::new().unwrap();
        let platform = current_platform();
        std::fs::write(
            tmp.path().join(MISE_CONFIG),
            r#"
[tools]
rust = "1.86"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join(MISE_LOCK),
            format!(
                r#"
[[tools.rust]]
version = "1.86.0"
backend = "core:rust"

[tools.rust.platforms.{}]
checksum = "sha256:abc"
size = 123
url = "https://example.test/rust.tar.gz"
"#,
                platform.mise
            ),
        )
        .unwrap();

        let view = inspect_workspace(tmp.path()).unwrap();
        let locked = view.tools[0].locked.as_ref().unwrap();

        assert_eq!(view.lock.as_deref(), Some(MISE_LOCK));
        assert_eq!(locked.version, "1.86.0");
        assert_eq!(locked.backend.as_deref(), Some("core:rust"));
        assert_eq!(
            locked.platform.as_ref().and_then(|p| p.checksum.as_deref()),
            Some("sha256:abc")
        );
    }

    #[test]
    fn fingerprint_changes_when_tool_request_changes() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(MISE_CONFIG),
            r#"
[tools]
rust = "1.86"
"#,
        )
        .unwrap();
        let before = inspect_workspace(tmp.path()).unwrap().fingerprint;

        std::fs::write(
            tmp.path().join(MISE_CONFIG),
            r#"
[tools]
rust = "1.87"
"#,
        )
        .unwrap();
        let after = inspect_workspace(tmp.path()).unwrap().fingerprint;

        assert_ne!(before, after);
    }
}
