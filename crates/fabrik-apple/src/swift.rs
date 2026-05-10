use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fabrik_cas::Digest;
use fabrik_core::{Action, PlanNode, ResourceRequest, WorkspacePath};
use fabrik_frontend::Target;

use crate::artifact::{
    executable_path, framework_path, parent_dir, swift_out_dir, swift_static_library_path,
    AppleKind, SwiftArtifact, SwiftImportSearch, SwiftLinkInput,
};

#[derive(Debug, Clone)]
pub struct SwiftPlan {
    pub nodes: Vec<SwiftPlanNode>,
    pub import_node: usize,
    pub artifact: SwiftArtifact,
}

#[derive(Debug, Clone)]
pub struct SwiftPlanNode {
    pub node: PlanNode,
    pub kind: String,
    pub target_dep_mode: TargetDepMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetDepMode {
    None,
    Root,
    Import,
}

#[derive(Debug, thiserror::Error)]
pub enum SwiftError {
    #[error("target {label} has unsupported kind `{kind}`")]
    UnsupportedKind { label: String, kind: String },
    #[error("target {label} has no Swift sources")]
    NoSources { label: String },
    #[error("target {label}: invalid path `{path}`: {source}")]
    InvalidPath {
        label: String,
        path: String,
        #[source]
        source: fabrik_core::WorkspacePathError,
    },
    #[error("target {label} declares dep `{dep}` that is not a known Swift/Apple target")]
    UnknownDep { label: String, dep: String },
    #[error("failed to read source `{path}` for target {label}: {source}")]
    ReadSource {
        label: String,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("target {label}: failed to parse `{attr}`: {source}")]
    ParseAttr {
        label: String,
        attr: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("host architecture `{0}` is not supported for macOS Swift builds")]
    UnsupportedHostArch(String),
}

pub fn compile_swift_target(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
) -> Result<SwiftPlan, SwiftError> {
    let kind = AppleKind::parse(&target.kind).ok_or_else(|| SwiftError::UnsupportedKind {
        label: target.label(),
        kind: target.kind.clone(),
    })?;
    match kind {
        AppleKind::SwiftLibrary => compile_swift_library(target, workspace_root, dep_artifacts),
        AppleKind::AppleStaticFramework | AppleKind::AppleDynamicFramework => {
            compile_framework(target, workspace_root, dep_artifacts, kind)
        }
        AppleKind::MacosCommandLineApplication => {
            compile_command_line_application(target, workspace_root, dep_artifacts)
        }
        AppleKind::IosApp => Err(SwiftError::UnsupportedKind {
            label: target.label(),
            kind: target.kind.clone(),
        }),
    }
}

fn compile_swift_library(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
) -> Result<SwiftPlan, SwiftError> {
    let kind = AppleKind::SwiftLibrary;
    require_sources(target)?;
    let module_name = module_name(target);
    let module_segment = sh_single_segment(&module_name);
    let minimum_os = minimum_os(target);
    let target_triple = macos_target_triple(&minimum_os)?;
    let out_dir = swift_out_dir(&target.package, &target.name);
    let static_library = swift_static_library_path(&out_dir, &module_name);
    let source_args = source_paths(target)?
        .iter()
        .map(|s| sh_workspace_root_path(s))
        .collect::<Vec<_>>()
        .join(" ");
    let swiftc_flags = swiftc_flags(target)?;
    let dep_import_args = dep_import_args(target, dep_artifacts)?;
    let compile_script = format!(
        r#"set -eu
ROOT="$(pwd)"
SDK="$(xcrun --sdk macosx --show-sdk-path)"
OUT_DIR={out_dir}
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"
(cd "$OUT_DIR" && SDKROOT="$SDK" xcrun --sdk macosx swiftc -sdk "$SDK" -target {target_triple} -parse-as-library -module-name {module_name} -emit-module -emit-module-path "{module_segment}.swiftmodule" -emit-object {dep_import_args}{swiftc_flags}{source_args})
"#,
        out_dir = sh_quote(&out_dir),
        target_triple = sh_quote(&target_triple),
        module_name = sh_quote(&module_name),
        module_segment = module_segment,
        dep_import_args = shell_import_args_from_root(&dep_import_args),
        swiftc_flags = shell_args(&swiftc_flags),
        source_args = source_args,
    );
    let input_digest = build_input_digest(target, workspace_root, dep_artifacts, kind)?;
    let output = workspace_path(target, &out_dir)?;
    let compile_action = Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), compile_script],
        env: tool_env(),
        cwd: None,
        input_digest: Some(input_digest),
        outputs: vec![output],
        resources: ResourceRequest::new(2, 0),
        timeout_ms: Some(300_000),
    };
    let compile_digest = compile_action.digest();
    let archive_script = format!(
        r#"set -eu
OUT_DIR={out_dir}
STATIC_LIBRARY={static_library}
rm -f "$STATIC_LIBRARY"
xcrun ar crs "$STATIC_LIBRARY" "$OUT_DIR"/*.o
"#,
        out_dir = sh_quote(&out_dir),
        static_library = sh_quote(&static_library),
    );
    let archive_input_digest =
        archive_input_digest(target, &module_name, &static_library, compile_digest);
    let archive_output = workspace_path(target, &static_library)?;
    let archive_action = Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), archive_script],
        env: tool_env(),
        cwd: None,
        input_digest: Some(archive_input_digest),
        outputs: vec![archive_output],
        resources: ResourceRequest::default(),
        timeout_ms: Some(120_000),
    };
    let archive_digest = archive_action.digest();
    let own_import = SwiftImportSearch::ModuleDir(out_dir.clone());
    let artifact = SwiftArtifact {
        module_name,
        import_search: own_import.clone(),
        import_searches: merge_import_searches(
            vec![own_import],
            direct_dep_import_searches(target, dep_artifacts)?,
        ),
        link_inputs: merge_link_inputs(
            vec![SwiftLinkInput::StaticArchive(static_library)],
            direct_dep_link_inputs(target, dep_artifacts)?,
        ),
        output: out_dir,
        action_digest: archive_digest,
        kind,
    };
    let mut archive_node = plan_node_with_label(target, "archive", archive_action);
    archive_node.deps = vec![0];
    Ok(SwiftPlan {
        nodes: vec![
            SwiftPlanNode {
                node: plan_node_with_label(target, "compile", compile_action),
                kind: "swift_compile".to_string(),
                target_dep_mode: TargetDepMode::Import,
            },
            SwiftPlanNode {
                node: archive_node,
                kind: "swift_archive".to_string(),
                target_dep_mode: TargetDepMode::None,
            },
        ],
        import_node: 0,
        artifact,
    })
}

fn compile_framework(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
    kind: AppleKind,
) -> Result<SwiftPlan, SwiftError> {
    require_sources(target)?;
    let module_name = module_name(target);
    let minimum_os = minimum_os(target);
    let target_triple = macos_target_triple(&minimum_os)?;
    let module_triple = macos_module_triple()?;
    let framework = framework_path(&target.package, &target.name);
    let parent = parent_dir(&framework);
    let binary = format!("{framework}/{module_name}");
    let source_args = source_paths(target)?
        .iter()
        .map(|s| sh_quote(s))
        .collect::<Vec<_>>()
        .join(" ");
    let swiftc_flags = swiftc_flags(target)?;
    let dep_import_args = dep_import_args(target, dep_artifacts)?;
    let emit_mode = match kind {
        AppleKind::AppleStaticFramework => "-emit-library -static",
        AppleKind::AppleDynamicFramework => "-emit-library -Xlinker -install_name -Xlinker @rpath/{module_name}.framework/{module_name}",
        _ => unreachable!("framework compiler called with non-framework kind"),
    };
    let plist = framework_info_plist(target, &module_name);
    let script = format!(
        r#"set -eu
SDK="$(xcrun --sdk macosx --show-sdk-path)"
FRAMEWORK={framework}
rm -rf "$FRAMEWORK"
mkdir -p "$FRAMEWORK/Modules/{module_name}.swiftmodule"
cat > "$FRAMEWORK/Info.plist" <<'FABRIK_PLIST'
{plist}
FABRIK_PLIST
SDKROOT="$SDK" xcrun --sdk macosx swiftc -sdk "$SDK" -target {target_triple} -parse-as-library -module-name {module_name} -emit-module -emit-module-path "$FRAMEWORK/Modules/{module_name}.swiftmodule/{module_triple}.swiftmodule" {emit_mode} -o {binary} {dep_import_args}{swiftc_flags}{source_args}
"#,
        framework = sh_quote(&framework),
        module_name = sh_single_segment(&module_name),
        plist = plist,
        target_triple = sh_quote(&target_triple),
        module_triple = sh_single_segment(&module_triple),
        emit_mode = emit_mode.replace("{module_name}", &sh_single_segment(&module_name)),
        binary = sh_quote(&binary),
        dep_import_args = shell_args(&dep_import_args),
        swiftc_flags = shell_args(&swiftc_flags),
        source_args = source_args,
    );
    let input_digest = build_input_digest(target, workspace_root, dep_artifacts, kind)?;
    let output = workspace_path(target, &framework)?;
    let action = Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script],
        env: tool_env(),
        cwd: None,
        input_digest: Some(input_digest),
        outputs: vec![output],
        resources: ResourceRequest::new(2, 0),
        timeout_ms: Some(300_000),
    };
    let action_digest = action.digest();
    let own_link = match kind {
        AppleKind::AppleStaticFramework => SwiftLinkInput::StaticFramework {
            name: module_name.clone(),
            parent: parent.clone(),
        },
        AppleKind::AppleDynamicFramework => SwiftLinkInput::DynamicFramework {
            name: module_name.clone(),
            parent: parent.clone(),
        },
        _ => unreachable!("framework compiler called with non-framework kind"),
    };
    let own_import = SwiftImportSearch::FrameworkParent(parent.clone());
    let artifact = SwiftArtifact {
        module_name,
        import_search: own_import.clone(),
        import_searches: merge_import_searches(
            vec![own_import],
            direct_dep_import_searches(target, dep_artifacts)?,
        ),
        link_inputs: merge_link_inputs(
            vec![own_link],
            direct_dep_link_inputs(target, dep_artifacts)?,
        ),
        output: framework,
        action_digest,
        kind,
    };
    Ok(SwiftPlan {
        nodes: vec![SwiftPlanNode {
            node: plan_node(target, action),
            kind: kind.as_str().to_string(),
            target_dep_mode: TargetDepMode::Root,
        }],
        import_node: 0,
        artifact,
    })
}

fn compile_command_line_application(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
) -> Result<SwiftPlan, SwiftError> {
    require_sources(target)?;
    let module_name = module_name(target);
    let minimum_os = minimum_os(target);
    let target_triple = macos_target_triple(&minimum_os)?;
    let executable = executable_path(&target.package, &target.name);
    let output_parent = parent_dir(&executable);
    let source_args = source_paths(target)?
        .iter()
        .map(|s| sh_quote(s))
        .collect::<Vec<_>>()
        .join(" ");
    let swiftc_flags = swiftc_flags(target)?;
    let dep_import_args = dep_import_args(target, dep_artifacts)?;
    let link_args = direct_dep_link_inputs(target, dep_artifacts)?
        .into_iter()
        .flat_map(|input| input.link_args())
        .collect::<Vec<_>>();
    let rpath_args = direct_dep_link_inputs(target, dep_artifacts)?
        .into_iter()
        .filter_map(|input| match input {
            SwiftLinkInput::DynamicFramework { parent, .. } => Some(parent),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .flat_map(|parent| {
            vec![
                "-Xlinker".to_string(),
                "-rpath".to_string(),
                "-Xlinker".to_string(),
                parent,
            ]
        })
        .collect::<Vec<_>>();
    let script = format!(
        r#"set -eu
SDK="$(xcrun --sdk macosx --show-sdk-path)"
mkdir -p {output_parent}
rm -f {executable}
SDKROOT="$SDK" xcrun --sdk macosx swiftc -sdk "$SDK" -target {target_triple} -module-name {module_name} {dep_import_args}{swiftc_flags}{source_args} {link_args}{rpath_args}-o {executable}
"#,
        output_parent = sh_quote(&output_parent),
        executable = sh_quote(&executable),
        target_triple = sh_quote(&target_triple),
        module_name = sh_quote(&module_name),
        dep_import_args = shell_args(&dep_import_args),
        swiftc_flags = shell_args(&swiftc_flags),
        source_args = source_args,
        link_args = shell_args(&link_args),
        rpath_args = shell_args(&rpath_args),
    );
    let input_digest = build_input_digest(
        target,
        workspace_root,
        dep_artifacts,
        AppleKind::MacosCommandLineApplication,
    )?;
    let output = workspace_path(target, &executable)?;
    let action = Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script],
        env: tool_env(),
        cwd: None,
        input_digest: Some(input_digest),
        outputs: vec![output],
        resources: ResourceRequest::new(2, 0),
        timeout_ms: Some(300_000),
    };
    let action_digest = action.digest();
    let own_import = SwiftImportSearch::ModuleDir(output_parent);
    let artifact = SwiftArtifact {
        module_name,
        import_search: own_import.clone(),
        import_searches: merge_import_searches(
            vec![own_import],
            direct_dep_import_searches(target, dep_artifacts)?,
        ),
        link_inputs: vec![SwiftLinkInput::StaticArchive(executable.clone())],
        output: executable,
        action_digest,
        kind: AppleKind::MacosCommandLineApplication,
    };
    Ok(SwiftPlan {
        nodes: vec![SwiftPlanNode {
            node: plan_node(target, action),
            kind: AppleKind::MacosCommandLineApplication.as_str().to_string(),
            target_dep_mode: TargetDepMode::Root,
        }],
        import_node: 0,
        artifact,
    })
}

fn require_sources(target: &Target) -> Result<(), SwiftError> {
    if target.srcs.is_empty() {
        return Err(SwiftError::NoSources {
            label: target.label(),
        });
    }
    Ok(())
}

fn module_name(target: &Target) -> String {
    target
        .attrs
        .get("module_name")
        .cloned()
        .unwrap_or_else(|| default_module_name(&target.name))
}

fn default_module_name(name: &str) -> String {
    let mut out = String::new();
    for (idx, c) in name.chars().enumerate() {
        let valid = c.is_ascii_alphanumeric() || c == '_';
        if idx == 0 && c.is_ascii_digit() {
            out.push('_');
        }
        out.push(if valid { c } else { '_' });
    }
    if out.is_empty() {
        "Module".to_string()
    } else {
        out
    }
}

fn minimum_os(target: &Target) -> String {
    target
        .attrs
        .get("minimum_os")
        .cloned()
        .unwrap_or_else(|| "15.0".to_string())
}

fn swiftc_flags(target: &Target) -> Result<Vec<String>, SwiftError> {
    match target.attrs.get("swiftc_flags_json") {
        Some(raw) => serde_json::from_str(raw).map_err(|source| SwiftError::ParseAttr {
            label: target.label(),
            attr: "swiftc_flags_json".to_string(),
            source,
        }),
        None => Ok(Vec::new()),
    }
}

fn source_paths(target: &Target) -> Result<Vec<String>, SwiftError> {
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
                .map_err(|source| SwiftError::InvalidPath {
                    label: target.label(),
                    path: rel,
                    source,
                })
        })
        .collect()
}

fn dep_import_args(
    target: &Target,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
) -> Result<Vec<String>, SwiftError> {
    let mut args = Vec::new();
    let mut seen = BTreeSet::new();
    for dep in &target.deps {
        let artifact = dep_artifacts
            .get(dep)
            .ok_or_else(|| SwiftError::UnknownDep {
                label: target.label(),
                dep: dep.clone(),
            })?;
        let key = match &artifact.import_search {
            SwiftImportSearch::ModuleDir(dir) => format!("I:{dir}"),
            SwiftImportSearch::FrameworkParent(parent) => format!("F:{parent}"),
        };
        if seen.insert(key) {
            args.extend(artifact.import_args());
        }
    }
    Ok(args)
}

fn direct_dep_link_inputs(
    target: &Target,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
) -> Result<Vec<SwiftLinkInput>, SwiftError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for dep in &target.deps {
        let artifact = dep_artifacts
            .get(dep)
            .ok_or_else(|| SwiftError::UnknownDep {
                label: target.label(),
                dep: dep.clone(),
            })?;
        for input in &artifact.link_inputs {
            if seen.insert(input.cache_key()) {
                out.push(input.clone());
            }
        }
    }
    Ok(out)
}

fn direct_dep_import_searches(
    target: &Target,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
) -> Result<Vec<SwiftImportSearch>, SwiftError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for dep in &target.deps {
        let artifact = dep_artifacts
            .get(dep)
            .ok_or_else(|| SwiftError::UnknownDep {
                label: target.label(),
                dep: dep.clone(),
            })?;
        for search in &artifact.import_searches {
            if seen.insert(search.cache_key()) {
                out.push(search.clone());
            }
        }
    }
    Ok(out)
}

fn merge_link_inputs(own: Vec<SwiftLinkInput>, deps: Vec<SwiftLinkInput>) -> Vec<SwiftLinkInput> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for input in own.into_iter().chain(deps) {
        if seen.insert(input.cache_key()) {
            out.push(input);
        }
    }
    out
}

fn merge_import_searches(
    own: Vec<SwiftImportSearch>,
    deps: Vec<SwiftImportSearch>,
) -> Vec<SwiftImportSearch> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for search in own.into_iter().chain(deps) {
        if seen.insert(search.cache_key()) {
            out.push(search);
        }
    }
    out
}

fn build_input_digest(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
    kind: AppleKind,
) -> Result<Digest, SwiftError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.apple.swift.input.v1\0");
    buf.extend_from_slice(kind.as_str().as_bytes());
    buf.push(0);
    buf.extend_from_slice(module_name(target).as_bytes());
    buf.push(0);
    buf.extend_from_slice(minimum_os(target).as_bytes());
    buf.push(0);
    for flag in swiftc_flags(target)? {
        buf.extend_from_slice(b"swiftc-flag:");
        buf.extend_from_slice(flag.as_bytes());
        buf.push(0);
    }

    let mut srcs = source_paths(target)?;
    srcs.sort();
    for src in srcs {
        let abs = workspace_root.join(&src);
        let bytes = std::fs::read(&abs).map_err(|source| SwiftError::ReadSource {
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

    let mut deps: Vec<&String> = target.deps.iter().collect();
    deps.sort();
    for dep in deps {
        if let Some(artifact) = dep_artifacts.get(dep) {
            buf.extend_from_slice(b"dep:");
            buf.extend_from_slice(dep.as_bytes());
            buf.push(0);
            buf.extend_from_slice(artifact.action_digest.as_bytes());
            buf.push(0);
        }
    }

    for key in ["DEVELOPER_DIR", "TOOLCHAINS"] {
        if let Ok(value) = std::env::var(key) {
            buf.extend_from_slice(key.as_bytes());
            buf.push(b'=');
            buf.extend_from_slice(value.as_bytes());
            buf.push(0);
        }
    }
    buf.extend_from_slice(b"target-triple:");
    buf.extend_from_slice(macos_target_triple(&minimum_os(target))?.as_bytes());
    buf.push(0);

    Ok(Digest::of_bytes(&buf))
}

fn archive_input_digest(
    target: &Target,
    module_name: &str,
    static_library: &str,
    compile_digest: Digest,
) -> Digest {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.apple.swift.archive.input.v1\0");
    buf.extend_from_slice(target.label().as_bytes());
    buf.push(0);
    buf.extend_from_slice(module_name.as_bytes());
    buf.push(0);
    buf.extend_from_slice(static_library.as_bytes());
    buf.push(0);
    buf.extend_from_slice(compile_digest.as_bytes());
    buf.push(0);
    Digest::of_bytes(&buf)
}

fn macos_target_triple(minimum_os: &str) -> Result<String, SwiftError> {
    Ok(format!("{}-apple-macosx{}", apple_arch()?, minimum_os))
}

fn macos_module_triple() -> Result<String, SwiftError> {
    Ok(format!("{}-apple-macos", apple_arch()?))
}

fn apple_arch() -> Result<&'static str, SwiftError> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("arm64"),
        "x86_64" => Ok("x86_64"),
        other => Err(SwiftError::UnsupportedHostArch(other.to_string())),
    }
}

fn workspace_path(target: &Target, path: &str) -> Result<WorkspacePath, SwiftError> {
    WorkspacePath::try_from(path).map_err(|source| SwiftError::InvalidPath {
        label: target.label(),
        path: path.to_string(),
        source,
    })
}

fn plan_node(target: &Target, action: Action) -> PlanNode {
    PlanNode {
        label: target.label(),
        action,
        deps: Vec::new(),
    }
}

fn plan_node_with_label(target: &Target, suffix: &str, action: Action) -> PlanNode {
    PlanNode {
        label: format!("{}#{suffix}", target.label()),
        action,
        deps: Vec::new(),
    }
}

fn framework_info_plist(target: &Target, module_name: &str) -> String {
    let bundle_id = target
        .attrs
        .get("bundle_id")
        .cloned()
        .unwrap_or_else(|| format!("dev.fabrik.{module_name}"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>{executable}</string>
  <key>CFBundleIdentifier</key>
  <string>{bundle_id}</string>
  <key>CFBundleName</key>
  <string>{name}</string>
  <key>CFBundlePackageType</key>
  <string>FMWK</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
</dict>
</plist>
"#,
        executable = xml_escape(module_name),
        bundle_id = xml_escape(&bundle_id),
        name = xml_escape(&target.name),
    )
}

fn shell_args(values: &[String]) -> String {
    if values.is_empty() {
        String::new()
    } else {
        values
            .iter()
            .map(|s| sh_quote(s))
            .collect::<Vec<_>>()
            .join(" ")
            + " "
    }
}

fn shell_import_args_from_root(values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let mut args = Vec::with_capacity(values.len());
    let mut expects_path = false;
    for value in values {
        if expects_path {
            args.push(sh_workspace_root_path(value));
            expects_path = false;
        } else {
            args.push(sh_quote(value));
            expects_path = value == "-I" || value == "-F";
        }
    }
    args.join(" ") + " "
}

fn tool_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in ["PATH", "HOME", "DEVELOPER_DIR", "SDKROOT", "TOOLCHAINS"] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    for (key, value) in std::env::vars() {
        if key.starts_with("MISE_") {
            env.insert(key, value);
        }
    }
    env
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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

fn sh_workspace_root_path(path: &str) -> String {
    format!("\"$ROOT/{}\"", sh_double_escape(path))
}

fn sh_double_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
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
    use tempfile::TempDir;

    fn write(workspace: &Path, rel: &str, body: &str) {
        let p = workspace.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn target(kind: &str, package: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: package.into(),
            kind: kind.into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn swift_library_emits_static_archive_and_module() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "Lib/Sources/Lib.swift",
            "public func value() {}",
        );
        let lib = target(
            "swift_library",
            "Lib",
            "Greeter",
            &["Sources/Lib.swift"],
            &[],
        );
        let plan = compile_swift_target(&lib, tmp.path(), &BTreeMap::new()).unwrap();
        assert_eq!(plan.nodes.len(), 2);
        let Action::RunCommand { argv, outputs, .. } = &plan.nodes[0].node.action;
        assert_eq!(argv[0], "/bin/sh");
        assert!(argv[2].contains("-emit-object"));
        assert!(argv[2].contains("-module-name 'Greeter'"));
        assert_eq!(outputs[0].as_str(), ".fabrik/out/Lib/Greeter");
        let Action::RunCommand {
            argv: archive_argv,
            outputs: archive_outputs,
            ..
        } = &plan.nodes[1].node.action;
        assert!(archive_argv[2].contains("xcrun ar crs"));
        assert_eq!(
            archive_outputs[0].as_str(),
            ".fabrik/out/Lib/Greeter/libGreeter.a"
        );
        assert_eq!(plan.artifact.module_name, "Greeter");
        assert_eq!(
            plan.artifact.link_inputs,
            vec![SwiftLinkInput::StaticArchive(
                ".fabrik/out/Lib/Greeter/libGreeter.a".to_string()
            )]
        );
    }

    #[test]
    fn command_line_app_links_transitive_static_libraries() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "Base/Base.swift", "public func base() {}");
        write(tmp.path(), "Greeter/Greeter.swift", "import Base");
        write(tmp.path(), "App/main.swift", "import Greeter");
        let base = target("swift_library", "Base", "Base", &["Base.swift"], &[]);
        let base_plan = compile_swift_target(&base, tmp.path(), &BTreeMap::new()).unwrap();
        let mut deps = BTreeMap::new();
        deps.insert("//Base:Base".to_string(), base_plan.artifact);
        let greeter = target(
            "swift_library",
            "Greeter",
            "Greeter",
            &["Greeter.swift"],
            &["//Base:Base"],
        );
        let greeter_plan = compile_swift_target(&greeter, tmp.path(), &deps).unwrap();
        assert!(greeter_plan
            .artifact
            .import_args()
            .contains(&".fabrik/out/Base/Base".to_string()));
        deps.insert("//Greeter:Greeter".to_string(), greeter_plan.artifact);
        let app = target(
            "macos_command_line_application",
            "App",
            "hello",
            &["main.swift"],
            &["//Greeter:Greeter"],
        );
        let app_plan = compile_swift_target(&app, tmp.path(), &deps).unwrap();
        let Action::RunCommand { argv, .. } = &app_plan.nodes[0].node.action;
        assert!(argv[2].contains("'.fabrik/out/Base/Base'"));
        assert!(argv[2].contains(".fabrik/out/Greeter/Greeter/libGreeter.a"));
        assert!(argv[2].contains(".fabrik/out/Base/Base/libBase.a"));
    }

    #[test]
    fn input_digest_changes_when_dep_changes() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "Lib/Lib.swift", "public func value() {}");
        write(tmp.path(), "App/main.swift", "import Lib");
        let lib = target("swift_library", "Lib", "Lib", &["Lib.swift"], &[]);
        let lib_v1 = compile_swift_target(&lib, tmp.path(), &BTreeMap::new())
            .unwrap()
            .artifact;
        write(tmp.path(), "Lib/Lib.swift", "public func value2() {}");
        let lib_v2 = compile_swift_target(&lib, tmp.path(), &BTreeMap::new())
            .unwrap()
            .artifact;
        assert_ne!(lib_v1.action_digest, lib_v2.action_digest);

        let app = target(
            "macos_command_line_application",
            "App",
            "app",
            &["main.swift"],
            &["//Lib:Lib"],
        );
        let mut deps_v1 = BTreeMap::new();
        deps_v1.insert("//Lib:Lib".to_string(), lib_v1);
        let mut deps_v2 = BTreeMap::new();
        deps_v2.insert("//Lib:Lib".to_string(), lib_v2);
        let node_v1 = compile_swift_target(&app, tmp.path(), &deps_v1)
            .unwrap()
            .nodes
            .remove(0)
            .node;
        let node_v2 = compile_swift_target(&app, tmp.path(), &deps_v2)
            .unwrap()
            .nodes
            .remove(0)
            .node;
        assert_ne!(node_v1.action.digest(), node_v2.action.digest());
    }
}
