//! Turns a graph target and capability into a cacheable [`Action`].
//!
//! The current build, run, and test scripts are local placeholders: they
//! materialize the artifact layout each Apple product kind is expected to
//! produce and record a manifest, without invoking Xcode. They are not a
//! bypass of Once's execution substrate: every capability runs as an
//! [`Action`] through `once_core::run_with_cache`, so it goes through the
//! action cache and content-addressed storage like any other action. Keeping
//! this concern separate from orchestration lets the script generation grow
//! into real toolchain invocations without turning the command module into a
//! monolith.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use once_core::{Action, OutputSymlinkMode, ResourceRequest, WorkspacePath};
use once_frontend::{AttrValue, GraphTarget};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct AppleArtifactManifest<'a> {
    target: &'a str,
    kind: &'a str,
    capability: &'a str,
    attrs: &'a BTreeMap<String, AttrValue>,
    deps: &'a [String],
    srcs: &'a [String],
}

/// Build the cacheable action that produces a capability's outputs.
pub(super) fn action_for(
    target: &GraphTarget,
    capability: &str,
    outputs: &[WorkspacePath],
) -> Result<Action> {
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

/// Workspace-relative outputs a capability declares for a target kind.
pub(super) fn output_paths(target: &GraphTarget, capability: &str) -> Result<Vec<WorkspacePath>> {
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
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
{write_manifest}
",
    )
}

fn library_build_script(target: &GraphTarget, manifest_json: &str, output_paths: &str) -> String {
    let product = product_name(target);
    let product_text = shell_quote(&product);
    let target_text = shell_quote(&target.label.id);
    let manifest_path = shell_quote(&format!("{}/manifest.json", build_root(target)));
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
    let binary_path = shell_quote(&format!("{}/{}.a", build_root(target), product));
    let swiftmodule_path = shell_quote(&format!("{}/{}.swiftmodule", build_root(target), product));
    let generated_sources_path = shell_quote(&format!("{}/generated_sources", build_root(target)));
    let prepare_outputs = prepare_outputs_script(output_paths);
    format!(
        r"set -eu
{prepare_outputs}
{write_manifest}
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
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
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
{write_manifest}
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
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
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
{write_manifest}
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
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
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
{write_manifest}
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
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
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
{write_manifest}
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
    let write_manifest = write_manifest_cmd(&manifest_path, manifest_json);
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
{write_manifest}
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
        .and_then(AttrValue::as_str)
        .unwrap_or(&target.label.name)
        .to_string()
}

/// Emit the command that writes the artifact manifest.
///
/// The manifest is single-quoted with the same escaping every other dynamic
/// value uses, rather than embedded in a heredoc. This keeps all generated
/// content on the quoted side of the shell: no manifest value can terminate
/// the script body or be interpreted by the shell. `manifest_path` is already
/// shell quoted by the caller.
fn write_manifest_cmd(manifest_path: &str, manifest_json: &str) -> String {
    format!(
        "printf '%s\\n' {} > {manifest_path}",
        shell_quote(manifest_json)
    )
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    // POSIX single-quoted strings treat every byte literally except `'`,
    // which is represented by closing, emitting an escaped quote, and reopening.
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

    fn with_product(kind: &str, name: &str, product: &str) -> GraphTarget {
        let mut target = graph_target(kind, name);
        target.attrs.insert(
            "product_name".to_string(),
            AttrValue::String(product.to_string()),
        );
        target
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("App'; echo pwn"), "'App'\"'\"'; echo pwn'");
    }

    #[test]
    fn shell_quote_handles_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn xml_escape_escapes_all_entities() {
        assert_eq!(
            xml_escape(r#"A&B<C>D"E'F"#),
            "A&amp;B&lt;C&gt;D&quot;E&apos;F"
        );
    }

    #[test]
    fn product_name_prefers_attr_then_label() {
        assert_eq!(
            product_name(&graph_target("apple_library", "AppCore")),
            "AppCore"
        );
        assert_eq!(
            product_name(&with_product("apple_library", "AppCore", "CoreKit")),
            "CoreKit"
        );
        // A non-string product_name attr falls back to the target name.
        let mut target = graph_target("apple_library", "AppCore");
        target
            .attrs
            .insert("product_name".to_string(), AttrValue::Integer(7));
        assert_eq!(product_name(&target), "AppCore");
    }

    #[test]
    fn application_script_uses_shell_quoted_dynamic_text() {
        let target = with_product("apple_application", "App", "Bad'; echo pwn");

        let script = application_build_script(&target, "{}", "'.once/out/apps/ios/App'");

        assert!(script.contains("printf 'app executable for %s\\n' 'apps/ios/App'"));
        assert!(!script.contains("app executable for apps/ios/App"));
        assert!(!script.contains("echo pwn\\n"));
    }

    #[test]
    fn library_script_quotes_dynamic_text() {
        let target = with_product("apple_library", "AppCore", "Bad'; echo pwn");

        let script = library_build_script(&target, "{}", "'.once/out/apps/ios/AppCore'");

        assert!(script.contains("printf 'archive for %s\\n' 'apps/ios/AppCore'"));
        assert!(script.contains("'.once/out/apps/ios/AppCore/Bad'\"'\"'; echo pwn.a'"));
        assert!(!script.contains("> .once/out/apps/ios/AppCore/Bad'; echo pwn.a"));
    }

    #[test]
    fn test_bundle_script_quotes_dynamic_text() {
        let target = with_product("apple_test_bundle", "AppTests", "Bad'; echo pwn");

        let script = test_bundle_build_script(&target, "{}", "'.once/out/apps/ios/AppTests'");

        assert!(script.contains("printf 'xctest binary for %s\\n' 'apps/ios/AppTests'"));
        assert!(!script.contains("echo pwn\\n"));
    }

    #[test]
    fn plist_values_are_xml_escaped_before_shell_quoting() {
        let target = with_product("apple_framework", "Framework", "A&B<\"'>");

        let script = framework_build_script(&target, "{}", "'.once/out/apps/ios/Framework'");

        assert!(script.contains("A&amp;B&lt;&quot;&apos;&gt;"));
        assert!(!script.contains("<string>A&B<\"'></string>"));
    }

    #[test]
    fn run_script_guards_on_built_bundle() {
        let target = graph_target("apple_application", "App");

        let script = run_script(&target, "{}", "'.once/out/apps/ios/App/run'");

        assert!(script.contains("test -d '.once/out/apps/ios/App/App.app'"));
        assert!(script.contains(r#""status":"launched""#));
        assert!(script.contains("> '.once/out/apps/ios/App/run/run.json'"));
    }

    #[test]
    fn test_script_guards_on_built_bundle_and_emits_results() {
        let target = graph_target("apple_test_bundle", "AppTests");

        let script = test_script(&target, "{}", "'.once/out/apps/ios/AppTests/test'");

        assert!(script.contains("test -d '.once/out/apps/ios/AppTests/AppTests.xctest'"));
        assert!(script.contains(r#""status":"passed""#));
        assert!(script.contains("> '.once/out/apps/ios/AppTests/test/test_results.json'"));
        assert!(script.contains("> '.once/out/apps/ios/AppTests/test/coverage.json'"));
    }

    #[test]
    fn prepare_outputs_creates_dirs_for_files_and_dirs() {
        let script = prepare_outputs_script("'.once/out/x' '.once/out/x/App.a'");
        assert!(script.contains(r#"*.a|*.json|*.plist|*.swiftmodule) mkdir -p "$(dirname "$p")""#));
        assert!(script.contains(r#"*) mkdir -p "$p""#));
    }

    fn output_strings(target: &GraphTarget, capability: &str) -> Vec<String> {
        output_paths(target, capability)
            .unwrap()
            .into_iter()
            .map(|path| path.as_str().to_string())
            .collect()
    }

    #[test]
    fn output_paths_cover_library_products() {
        assert_eq!(
            output_strings(&graph_target("apple_library", "AppCore"), "build"),
            vec![
                ".once/out/apps/ios/AppCore".to_string(),
                ".once/out/apps/ios/AppCore/AppCore.a".to_string(),
                ".once/out/apps/ios/AppCore/AppCore.swiftmodule".to_string(),
                ".once/out/apps/ios/AppCore/generated_sources".to_string(),
            ]
        );
    }

    #[test]
    fn output_paths_cover_bundle_products() {
        assert_eq!(
            output_strings(&graph_target("apple_framework", "DesignSystem"), "build"),
            vec![
                ".once/out/apps/ios/DesignSystem".to_string(),
                ".once/out/apps/ios/DesignSystem/DesignSystem.framework".to_string(),
                ".once/out/apps/ios/DesignSystem/dSYMs".to_string(),
            ]
        );
        assert_eq!(
            output_strings(&graph_target("apple_application", "App"), "build"),
            vec![
                ".once/out/apps/ios/App".to_string(),
                ".once/out/apps/ios/App/App.app".to_string(),
                ".once/out/apps/ios/App/dSYMs".to_string(),
            ]
        );
        assert_eq!(
            output_strings(&graph_target("apple_test_bundle", "AppTests"), "build"),
            vec![
                ".once/out/apps/ios/AppTests".to_string(),
                ".once/out/apps/ios/AppTests/AppTests.xctest".to_string(),
                ".once/out/apps/ios/AppTests/dSYMs".to_string(),
            ]
        );
    }

    #[test]
    fn output_paths_cover_run_and_test() {
        assert_eq!(
            output_strings(&graph_target("apple_application", "App"), "run"),
            vec![".once/out/apps/ios/App/run".to_string()]
        );
        assert_eq!(
            output_strings(&graph_target("apple_test_bundle", "AppTests"), "test"),
            vec![
                ".once/out/apps/ios/AppTests/test".to_string(),
                ".once/out/apps/ios/AppTests/test/test_results.json".to_string(),
                ".once/out/apps/ios/AppTests/test/coverage.json".to_string(),
            ]
        );
    }

    #[test]
    fn output_paths_use_product_name_when_set() {
        assert_eq!(
            output_strings(&with_product("apple_application", "App", "Once"), "build"),
            vec![
                ".once/out/apps/ios/App".to_string(),
                ".once/out/apps/ios/App/Once.app".to_string(),
                ".once/out/apps/ios/App/dSYMs".to_string(),
            ]
        );
    }

    #[test]
    fn output_paths_reject_unknown_capability() {
        let err = output_paths(&graph_target("apple_library", "AppCore"), "lint")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported graph capability `lint`"));
    }

    #[test]
    fn output_paths_reject_product_name_that_escapes_workspace() {
        // A product_name carrying `..` would otherwise steer generated paths
        // (and the chmod target) outside the workspace. WorkspacePath rejects
        // the escape before any script is built or run.
        let target = with_product("apple_application", "App", "../../etc/App");
        let err = output_paths(&target, "build").unwrap_err().to_string();
        assert!(err.contains("invalid graph output path"));
    }

    #[test]
    fn action_for_wraps_script_in_sh_invocation() {
        let target = graph_target("apple_library", "AppCore");
        let outputs = output_paths(&target, "build").unwrap();
        let Action::RunCommand {
            argv,
            outputs: action_outputs,
            input_digest,
            ..
        } = action_for(&target, "build", &outputs).unwrap();
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert!(argv[2].contains("manifest.json"));
        assert_eq!(action_outputs, outputs);
        assert!(input_digest.is_none());
    }

    #[test]
    fn manifest_is_single_quoted_not_heredoc() {
        let target = graph_target("apple_library", "AppCore");
        let outputs = output_paths(&target, "build").unwrap();
        let script = action_script(&target, "build", &outputs).unwrap();

        // The manifest is written through the same single-quote escaping as
        // every other value, so no heredoc terminator can appear in the body.
        assert!(!script.contains("ONCE_MANIFEST"));
        assert!(!script.contains("<<"));
        assert!(script.contains("printf '%s\\n' '"));
        assert!(script.contains("> '.once/out/apps/ios/AppCore/manifest.json'"));
    }

    #[test]
    fn write_manifest_cmd_single_quotes_dynamic_content() {
        // A manifest value that would close a quoted heredoc, plus a single
        // quote, must stay inert inside the generated command.
        let cmd = write_manifest_cmd("'out/manifest.json'", "{\n\"k\": \"ONCE_MANIFEST'x\"\n}");
        assert!(cmd.starts_with("printf '%s\\n' '"));
        assert!(cmd.ends_with("> 'out/manifest.json'"));
        // The embedded single quote is escaped via the close/escape/reopen form.
        assert!(cmd.contains("'\"'\"'"));
    }

    #[test]
    fn action_script_rejects_unknown_capability() {
        let target = graph_target("apple_library", "AppCore");
        let err = action_script(&target, "lint", &[]).unwrap_err().to_string();
        assert!(err.contains("unsupported graph capability `lint`"));
    }
}
