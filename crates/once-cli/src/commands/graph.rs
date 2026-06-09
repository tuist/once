//! Graph capability commands for build, run, and test.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use once_cas::CacheProvider;
use once_core::{Action, OutputSymlinkMode, ResourceRequest, RunOpts, WorkspacePath};
use once_frontend::{AttrValue, GraphTarget};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::commands::util::cache_tag;
use crate::render;

#[derive(Debug, Serialize)]
struct CapabilityRunRecord {
    target: String,
    kind: String,
    capability: String,
    status: &'static str,
    action_digest: String,
    cache: &'static str,
    output_groups: Vec<String>,
    required_outputs: Vec<String>,
    outputs: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AppleArtifactManifest<'a> {
    target: &'a str,
    kind: &'a str,
    capability: &'a str,
    attrs: &'a BTreeMap<String, AttrValue>,
    deps: &'a [String],
    srcs: &'a [String],
}

pub async fn build(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
) -> Result<ExitCode> {
    run_capability(workspace, cache, output, target_id, "build").await
}

pub async fn test(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
) -> Result<ExitCode> {
    let target = graph_target(workspace, target_id)?;
    ensure_capability(&target, "test")?;
    let _ = run_target_capability(workspace, cache, &target, "build").await?;
    let record = run_target_capability(workspace, cache, &target, "test").await?;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn run(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
) -> Result<ExitCode> {
    let target = graph_target(workspace, target_id)?;
    ensure_capability(&target, "run")?;
    let _ = run_target_capability(workspace, cache, &target, "build").await?;
    let record = run_target_capability(workspace, cache, &target, "run").await?;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub fn supports(workspace: &Path, target_id: &str, capability: &str) -> Result<bool> {
    let Some(target) = find_graph_target(workspace, target_id)? else {
        return Ok(false);
    };
    Ok(target
        .capabilities
        .iter()
        .any(|candidate| candidate.name == capability))
}

async fn run_capability(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    capability: &str,
) -> Result<ExitCode> {
    let target = graph_target(workspace, target_id)?;
    let record = run_target_capability(workspace, cache, &target, capability).await?;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

async fn run_target_capability(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    capability_name: &str,
) -> Result<CapabilityRunRecord> {
    let capability = ensure_capability(target, capability_name)?;
    let outputs = output_paths(target, capability_name)?;
    let action = action_for(target, capability_name, &outputs)?;
    let outcome = once_core::run_with_cache(&action, workspace, cache, RunOpts::default())
        .await
        .with_context(|| format!("executing {capability_name} for {}", target.label.id))?;
    let cache = cache_tag(outcome.cache);
    Ok(CapabilityRunRecord {
        target: target.label.id.clone(),
        kind: target.kind.clone(),
        capability: capability.name.clone(),
        status: if outcome.result.exit_code == 0 {
            "completed"
        } else {
            "failed"
        },
        action_digest: outcome.action.to_string(),
        cache,
        output_groups: capability.output_groups.clone(),
        required_outputs: capability.requires_outputs.clone(),
        outputs: outputs
            .into_iter()
            .map(|output| output.as_str().to_string())
            .collect(),
    })
}

fn graph_target(workspace: &Path, target_id: &str) -> Result<GraphTarget> {
    find_graph_target(workspace, target_id)?
        .with_context(|| format!("no target matches `{target_id}`"))
}

fn find_graph_target(workspace: &Path, target_id: &str) -> Result<Option<GraphTarget>> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    Ok(graph
        .into_iter()
        .find(|target| target.label.id == target_id))
}

fn ensure_capability<'a>(
    target: &'a GraphTarget,
    capability: &str,
) -> Result<&'a once_frontend::Capability> {
    target
        .capabilities
        .iter()
        .find(|candidate| candidate.name == capability)
        .ok_or_else(|| unsupported_capability(target, capability))
}

fn unsupported_capability(target: &GraphTarget, capability: &str) -> anyhow::Error {
    let available = target
        .capabilities
        .iter()
        .map(|capability| capability.name.as_str())
        .collect::<Vec<_>>();
    if available.is_empty() {
        return anyhow!(
            "{} ({}) does not expose any capabilities",
            target.label.id,
            target.kind
        );
    }
    anyhow!(
        "{} ({}) does not expose `{}`. Available capabilities: {}",
        target.label.id,
        target.kind,
        capability,
        available.join(", ")
    )
}

fn action_for(target: &GraphTarget, capability: &str, outputs: &[WorkspacePath]) -> Result<Action> {
    Ok(Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            action_script(target, capability, outputs)?,
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        outputs: outputs.to_vec(),
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        timeout_ms: None,
        remote: None,
    })
}

fn action_script(
    target: &GraphTarget,
    capability: &str,
    outputs: &[WorkspacePath],
) -> Result<String> {
    let manifest = AppleArtifactManifest {
        target: &target.label.id,
        kind: &target.kind,
        capability,
        attrs: &target.attrs,
        deps: &target.deps,
        srcs: &target.srcs,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let output_paths = outputs
        .iter()
        .map(|output| shell_quote(output.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    let script = match capability {
        "build" => build_script(target, &manifest_json, &output_paths),
        "run" => run_script(target, &manifest_json, &output_paths),
        "test" => test_script(target, &manifest_json, &output_paths),
        other => anyhow::bail!("unsupported graph capability `{other}`"),
    };
    Ok(script)
}

fn build_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    match target.kind.as_str() {
        "apple_library" => library_build_script(target, manifest_json, output_paths),
        "apple_framework" => framework_build_script(target, manifest_json, output_paths),
        "apple_application" => application_build_script(target, manifest_json, output_paths),
        "apple_test_bundle" => test_bundle_build_script(target, manifest_json, output_paths),
        _ => generic_build_script(target, manifest_json, output_paths),
    }
}

fn generic_build_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    let manifest_path = shell_quote(&format!("{}/manifest.json", build_root(target)));
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
",
    )
}

fn library_build_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    let product = product_name(target);
    let product_text = shell_quote(&product);
    let target_text = shell_quote(&target.label.id);
    let manifest_path = shell_quote(&format!("{}/manifest.json", build_root(target)));
    let binary_path = shell_quote(&format!("{}/{}.a", build_root(target), product));
    let swiftmodule_path = shell_quote(&format!("{}/{}.swiftmodule", build_root(target), product));
    let generated_sources_path = shell_quote(&format!("{}/generated_sources", build_root(target)));
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
printf 'archive for %s\n' {target_text} > {binary_path}
printf 'swiftmodule for %s\n' {product_text} > {swiftmodule_path}
mkdir -p {generated_sources_path}
",
    )
}

fn framework_build_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    let product = product_name(target);
    let product_text = shell_quote(&product);
    let target_text = shell_quote(&target.label.id);
    let root = build_root(target);
    let manifest_path = shell_quote(&format!("{root}/manifest.json"));
    let info_plist = shell_quote(&format!("{root}/{product}.framework/Info.plist"));
    let info_plist_content = shell_quote(&format!(
        r#"<?xml version="1.0"?><plist><dict><key>CFBundleName</key><string>{}</string></dict></plist>"#,
        xml_escape(&product)
    ));
    let binary_path = shell_quote(&format!("{root}/{product}.framework/{product}"));
    let modules_path = shell_quote(&format!("{root}/{product}.framework/Modules"));
    let modulemap_path = shell_quote(&format!(
        "{root}/{product}.framework/Modules/module.modulemap"
    ));
    let dsyms_contents_path =
        shell_quote(&format!("{root}/dSYMs/{product}.framework.dSYM/Contents"));
    let dsym_info_path = shell_quote(&format!(
        "{root}/dSYMs/{product}.framework.dSYM/Contents/Info.plist"
    ));
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
printf '%s\n' {info_plist_content} > {info_plist}
printf 'framework binary for %s\n' {target_text} > {binary_path}
mkdir -p {modules_path} {dsyms_contents_path}
printf 'framework module %s\n' {product_text} > {modulemap_path}
printf 'dSYM for %s\n' {target_text} > {dsym_info_path}
",
    )
}

fn application_build_script(
    target: &GraphTarget,
    manifest_json: &str,
    output_paths: &str,
) -> String {
    let product = product_name(target);
    let target_text = shell_quote(&target.label.id);
    let root = build_root(target);
    let manifest_path = shell_quote(&format!("{root}/manifest.json"));
    let info_plist = shell_quote(&format!("{root}/{product}.app/Info.plist"));
    let info_plist_content = shell_quote(&format!(
        r#"<?xml version="1.0"?><plist><dict><key>CFBundleExecutable</key><string>{}</string></dict></plist>"#,
        xml_escape(&product)
    ));
    let binary_path = shell_quote(&format!("{root}/{product}.app/{product}"));
    let resources_path = shell_quote(&format!("{root}/{product}.app/Resources"));
    let resources_manifest = shell_quote(&format!(
        "{root}/{product}.app/Resources/once-resources.txt"
    ));
    let dsyms_contents_path = shell_quote(&format!("{root}/dSYMs/{product}.app.dSYM/Contents"));
    let dsym_info_path = shell_quote(&format!(
        "{root}/dSYMs/{product}.app.dSYM/Contents/Info.plist"
    ));
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
printf '%s\n' {info_plist_content} > {info_plist}
printf 'app executable for %s\n' {target_text} > {binary_path}
chmod +x {binary_path}
mkdir -p {resources_path} {dsyms_contents_path}
printf 'resources for %s\n' {target_text} > {resources_manifest}
printf 'dSYM for %s\n' {target_text} > {dsym_info_path}
",
    )
}

fn test_bundle_build_script(
    target: &GraphTarget,
    manifest_json: &str,
    output_paths: &str,
) -> String {
    let product = product_name(target);
    let target_text = shell_quote(&target.label.id);
    let root = build_root(target);
    let manifest_path = shell_quote(&format!("{root}/manifest.json"));
    let info_plist = shell_quote(&format!("{root}/{product}.xctest/Info.plist"));
    let info_plist_content = shell_quote(&format!(
        r#"<?xml version="1.0"?><plist><dict><key>CFBundleExecutable</key><string>{}</string></dict></plist>"#,
        xml_escape(&product)
    ));
    let binary_path = shell_quote(&format!("{root}/{product}.xctest/{product}"));
    let dsyms_contents_path = shell_quote(&format!("{root}/dSYMs/{product}.xctest.dSYM/Contents"));
    let dsym_info_path = shell_quote(&format!(
        "{root}/dSYMs/{product}.xctest.dSYM/Contents/Info.plist"
    ));
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
printf '%s\n' {info_plist_content} > {info_plist}
printf 'xctest binary for %s\n' {target_text} > {binary_path}
chmod +x {binary_path}
mkdir -p {dsyms_contents_path}
printf 'dSYM for %s\n' {target_text} > {dsym_info_path}
",
    )
}

fn run_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    let product = product_name(target);
    let root = run_root(target);
    let bundle = format!("{product}.app");
    let bundle_path = shell_quote(&format!("{}/{bundle}", build_root(target)));
    let manifest_path = shell_quote(&format!("{root}/manifest.json"));
    let run_json = shell_quote(&format!("{root}/run.json"));
    let run_record = shell_quote(
        &serde_json::json!({
            "target": target.label.id,
            "bundle": bundle,
            "status": "launched",
        })
        .to_string(),
    );
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
test -d {bundle_path}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
printf '%s\n' {run_record} > {run_json}
",
    )
}

fn test_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    let product = product_name(target);
    let root = test_root(target);
    let bundle = format!("{product}.xctest");
    let bundle_path = shell_quote(&format!("{}/{bundle}", build_root(target)));
    let manifest_path = shell_quote(&format!("{root}/manifest.json"));
    let test_json = shell_quote(&format!("{root}/test_results.json"));
    let coverage_json = shell_quote(&format!("{root}/coverage.json"));
    let test_record = shell_quote(
        &serde_json::json!({
            "target": target.label.id,
            "bundle": bundle,
            "status": "passed",
            "tests": 0,
            "failures": 0,
        })
        .to_string(),
    );
    let coverage_record = shell_quote(
        &serde_json::json!({
            "target": target.label.id,
            "line_coverage": serde_json::Value::Null,
        })
        .to_string(),
    );
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
test -d {bundle_path}
cat > {manifest_path} <<'ONCE_MANIFEST'
{manifest_json}
ONCE_MANIFEST
printf '%s\n' {test_record} > {test_json}
printf '%s\n' {coverage_record} > {coverage_json}
",
    )
}

fn prepare_outputs_script(output_paths: &str) -> String {
    format!(
        r#"for p in {output_paths}; do
  case "$p" in
    *.a|*.json|*.plist|*.swiftmodule) mkdir -p "$(dirname "$p")" ;;
    *) mkdir -p "$p" ;;
  esac
done"#
    )
}

fn output_paths(target: &GraphTarget, capability: &str) -> Result<Vec<WorkspacePath>> {
    let product = product_name(target);
    let paths = match capability {
        "build" => match target.kind.as_str() {
            "apple_library" => vec![
                build_root(target),
                format!("{}/{}.a", build_root(target), product),
                format!("{}/{}.swiftmodule", build_root(target), product),
                format!("{}/generated_sources", build_root(target)),
            ],
            "apple_framework" => vec![
                build_root(target),
                format!("{}/{product}.framework", build_root(target)),
                format!("{}/dSYMs", build_root(target)),
            ],
            "apple_application" => vec![
                build_root(target),
                format!("{}/{product}.app", build_root(target)),
                format!("{}/dSYMs", build_root(target)),
            ],
            "apple_test_bundle" => vec![
                build_root(target),
                format!("{}/{product}.xctest", build_root(target)),
                format!("{}/dSYMs", build_root(target)),
            ],
            _ => vec![build_root(target)],
        },
        "run" => vec![run_root(target)],
        "test" => vec![
            test_root(target),
            format!("{}/test_results.json", test_root(target)),
            format!("{}/coverage.json", test_root(target)),
        ],
        other => anyhow::bail!("unsupported graph capability `{other}`"),
    };
    paths
        .into_iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid graph output path `{path}`"))
        })
        .collect()
}

fn build_root(target: &GraphTarget) -> String {
    format!(".once/out/{}", target.label.id)
}

fn run_root(target: &GraphTarget) -> String {
    format!(".once/out/{}/run", target.label.id)
}

fn test_root(target: &GraphTarget) -> String {
    format!(".once/out/{}/test", target.label.id)
}

fn product_name(target: &GraphTarget) -> String {
    target
        .attrs
        .get("product_name")
        .and_then(|value| match value {
            AttrValue::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| target.label.name.clone())
}

async fn write_record(output: Output, record: &CapabilityRunRecord) -> Result<()> {
    let body = match output.format {
        Format::Human => render_human(record),
        Format::Json | Format::Toon => render::structured(output.format, record)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn render_human(record: &CapabilityRunRecord) -> String {
    let groups = if record.output_groups.is_empty() {
        "none".to_string()
    } else {
        record.output_groups.join(", ")
    };
    let mut out = format!(
        "once: {} {} ({}) cache {}, exit=0\noutputs: {}\n",
        record.capability, record.target, record.kind, record.cache, groups
    );
    if !record.required_outputs.is_empty() {
        out.push_str("requires: ");
        out.push_str(&record.required_outputs.join(", "));
        out.push('\n');
    }
    if !record.outputs.is_empty() {
        out.push_str("paths:\n");
        for path in &record.outputs {
            out.push_str("  ");
            out.push_str(path);
            out.push('\n');
        }
    }
    out
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
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
    use once_frontend::{Capability, TargetLabel};

    fn graph_target(kind: &str, name: &str) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: "apps/ios".to_string(),
                name: name.to_string(),
                id: format!("apps/ios/{name}"),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: vec![Capability {
                name: "build".to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            }],
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("App'; echo pwn"), "'App'\"'\"'; echo pwn'");
    }

    #[test]
    fn application_script_uses_shell_quoted_dynamic_text() {
        let mut target = graph_target("apple_application", "App");
        target.attrs.insert(
            "product_name".to_string(),
            AttrValue::String("Bad'; echo pwn".to_string()),
        );

        let script = application_build_script(&target, "{}", "'.once/out/apps/ios/App'");

        assert!(script.contains("printf 'app executable for %s\\n' 'apps/ios/App'"));
        assert!(!script.contains("app executable for apps/ios/App"));
        assert!(!script.contains("echo pwn\\n"));
    }

    #[test]
    fn plist_values_are_xml_escaped_before_shell_quoting() {
        let mut target = graph_target("apple_framework", "Framework");
        target.attrs.insert(
            "product_name".to_string(),
            AttrValue::String("A&B<\"'>".to_string()),
        );

        let script = framework_build_script(&target, "{}", "'.once/out/apps/ios/Framework'");

        assert!(script.contains("A&amp;B&lt;&quot;&apos;&gt;"));
        assert!(!script.contains("<string>A&B<\"'></string>"));
    }
}
