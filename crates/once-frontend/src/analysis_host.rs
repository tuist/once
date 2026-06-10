//! Host-side helpers invoked by analysis-time starlark globals.
//!
//! Rule impls call `xcrun_swiftc(platform)` and `apple_triple(...)`
//! through starlark; the implementations live here so they can shell
//! out to `xcrun` and inspect the host architecture, which is not
//! safely expressible in starlark.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

/// Resolved swift toolchain inputs surfaced to the prelude.
#[derive(Debug, Clone)]
pub struct SwiftToolchainResolution {
    pub xcrun: String,
    pub sdk: String,
    pub identity: String,
}

/// Resolve the toolchain for a platform string declared in a target's
/// `platform` attribute. Spawns `xcrun --find swiftc` plus a version
/// query, so the cost is paid once per analyzed target.
pub fn resolve_swift_toolchain(platform: &str) -> Result<SwiftToolchainResolution> {
    if !cfg!(target_os = "macos") {
        bail!("apple_library targets require xcrun, which is only available on macOS");
    }
    let sdk = sdk_name(platform)?.to_string();
    let xcrun_path =
        which_xcrun().context("`xcrun` not found on PATH; install Xcode command line tools")?;
    let swiftc_path = run_capture(&xcrun_path, &["--sdk", &sdk, "--find", "swiftc"])
        .with_context(|| format!("resolving swiftc for sdk `{sdk}`"))?;
    let version = run_capture(&xcrun_path, &["--sdk", &sdk, "swiftc", "--version"])
        .with_context(|| format!("querying swiftc version for sdk `{sdk}`"))?;
    let identity = format!(
        "once.apple.swiftc.v1\0{}\0{}",
        swiftc_path.trim(),
        version.trim()
    );
    Ok(SwiftToolchainResolution {
        xcrun: xcrun_path.display().to_string(),
        sdk,
        identity,
    })
}

/// Render an LLVM triple for `platform`/`minimum_os` against the host
/// architecture. Used by prelude impls to populate `-target`.
#[must_use]
pub fn apple_triple(platform: &str, minimum_os: &str) -> String {
    let (os, suffix) = triple_parts(platform);
    format!("{arch}-apple-{os}{minimum_os}{suffix}", arch = host_arch(),)
}

fn sdk_name(platform: &str) -> Result<&'static str> {
    Ok(match platform {
        "ios" => "iphonesimulator",
        "macos" | "macosx" => "macosx",
        "tvos" => "appletvsimulator",
        "watchos" => "watchsimulator",
        "visionos" | "xros" => "xrsimulator",
        other => {
            return Err(anyhow!(
                "unsupported apple platform `{other}` (expected ios, macos, tvos, watchos, or visionos)"
            ));
        }
    })
}

fn triple_parts(platform: &str) -> (String, &'static str) {
    match platform {
        "macos" | "macosx" => ("macosx".to_string(), ""),
        "ios" => ("ios".to_string(), "-simulator"),
        "tvos" => ("tvos".to_string(), "-simulator"),
        "watchos" => ("watchos".to_string(), "-simulator"),
        "visionos" | "xros" => ("xros".to_string(), "-simulator"),
        // Unknown platforms render a clearly-wrong triple; the swiftc
        // invocation will reject it with an actionable diagnostic,
        // which is louder than silently swapping a default.
        other => (other.to_string(), ""),
    }
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        std::env::consts::ARCH
    }
}

fn which_xcrun() -> Result<PathBuf> {
    for entry in std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = entry.join("xcrun");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(anyhow!("`xcrun` not found on PATH"))
}

fn run_capture(command: &PathBuf, args: &[&str]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("executing `{}`", command.display()))?;
    if !output.status.success() {
        bail!(
            "`{} {}` exited with {}: {}",
            command.display(),
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    String::from_utf8(output.stdout).context("toolchain output was not utf-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ios_picks_simulator_triple_suffix() {
        let triple = apple_triple("ios", "17.0");
        assert!(triple.ends_with("-apple-ios17.0-simulator"));
    }

    #[test]
    fn macos_uses_macosx_triple_without_simulator_suffix() {
        let triple = apple_triple("macos", "14.0");
        assert!(triple.ends_with("-apple-macosx14.0"));
        assert!(!triple.contains("-simulator"));
    }

    #[test]
    fn sdk_name_rejects_unknown_platforms() {
        let err = sdk_name("linux").unwrap_err().to_string();
        assert!(err.contains("unsupported apple platform `linux`"));
    }
}
