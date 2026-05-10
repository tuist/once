//! `fabrik vendor` - generate per-crate `fabrik.toml` declarations
//! from a project's `Cargo.lock` so the granular pipeline can
//! compile third-party deps directly with rustc.
//!
//! What this verb does:
//! - Runs `cargo metadata --format-version 1` to resolve the dep
//!   graph (resolution, features, source locations).
//! - Walks every crates.io package in the resolve and emits a Rust
//!   target declaration into `vendor/fabrik.toml`.
//! - Emits commented-out stubs for crates that have a `build.rs` or
//!   that pull a build script in via build-deps. Those need
//!   primitives the granular pipeline does not yet thread end to
//!   end (see AGENTS.md, "Build script support").
//!
//! What this verb does **not** do:
//! - Vendor sources into the project. The generated declarations
//!   keep empty `srcs` placeholders until source copying exists.
//! - Resolve features per-target. The generator emits the global
//!   feature set; a richer model picks features per dependent.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::cli::Format;

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
    resolve: Option<MetadataResolve>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct MetadataPackage {
    name: String,
    version: String,
    id: String,
    manifest_path: String,
    /// `null` for path/git deps, `"registry+https://..."` for crates.io.
    source: Option<String>,
    targets: Vec<MetadataTarget>,
    /// Cargo flags this when the manifest declares a build script,
    /// including the implicit one via `build = "build.rs"`.
    #[serde(default)]
    build_dependencies: Vec<MetadataDependency>,
}

#[derive(Deserialize)]
struct MetadataTarget {
    #[allow(dead_code)]
    name: String,
    kind: Vec<String>,
    src_path: String,
    edition: String,
}

#[derive(Deserialize, Debug)]
struct MetadataDependency {
    #[allow(dead_code)]
    name: String,
}

#[derive(Deserialize)]
struct MetadataResolve {
    nodes: Vec<MetadataResolveNode>,
}

#[derive(Deserialize)]
struct MetadataResolveNode {
    id: String,
    deps: Vec<MetadataResolveDep>,
}

#[derive(Deserialize)]
struct MetadataResolveDep {
    name: String,
    pkg: String,
    /// Per-edge `dep_kinds`; an empty `kind` means "normal" dep.
    dep_kinds: Vec<MetadataDepKind>,
}

#[derive(Deserialize)]
struct MetadataDepKind {
    /// One of: `null` (normal), `"dev"`, `"build"`.
    #[serde(default)]
    kind: Option<String>,
}

pub async fn vendor(workspace: &Path, format: Format) -> Result<ExitCode> {
    let metadata = run_cargo_metadata(workspace).await?;
    let report = plan_vendor(&metadata)?;

    let vendor_dir = workspace.join("vendor");
    tokio::fs::create_dir_all(&vendor_dir)
        .await
        .with_context(|| format!("creating {}", vendor_dir.display()))?;
    let manifest = vendor_dir.join("fabrik.toml");
    tokio::fs::write(&manifest, &report.manifest_body)
        .await
        .with_context(|| format!("writing {}", manifest.display()))?;

    match format {
        Format::Human => {
            let mut err = tokio::io::stderr();
            let summary = format!(
                "fabrik: vendor wrote {path} ({declared} declared, {skipped} skipped)\n  declared: pure-rust libs and proc-macros without build scripts\n  skipped:  crates with build.rs (need cargo.build_script wiring)\n  cargo.binary remains the production path until the third-party graph is feature-complete\n",
                path = manifest.display(),
                declared = report.declared,
                skipped = report.skipped,
            );
            err.write_all(summary.as_bytes()).await?;
            err.flush().await?;
        }
        Format::Json | Format::Toon => {
            let mut out = tokio::io::stdout();
            let payload = serde_json::json!({
                "manifest": manifest.display().to_string(),
                "declared": report.declared,
                "skipped": report.skipped,
                "skipped_crates": report.skipped_names,
            });
            let body = serde_json::to_string(&payload)? + "\n";
            out.write_all(body.as_bytes()).await?;
            out.flush().await?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

async fn run_cargo_metadata(workspace: &Path) -> Result<CargoMetadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(workspace)
        .output()
        .await
        .context("spawning cargo metadata")?;
    if !output.status.success() {
        return Err(anyhow!(
            "cargo metadata failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).context("parsing cargo metadata JSON")
}

struct VendorReport {
    manifest_body: String,
    declared: usize,
    skipped: usize,
    skipped_names: Vec<String>,
}

fn plan_vendor(metadata: &CargoMetadata) -> Result<VendorReport> {
    let pkg_by_id: BTreeMap<&String, &MetadataPackage> =
        metadata.packages.iter().map(|p| (&p.id, p)).collect();
    let workspace_ids: BTreeSet<&String> = metadata.workspace_members.iter().collect();

    let normal_deps = resolve_normal_deps(metadata);

    let mut body = String::new();
    body.push_str(
        "# Generated by `fabrik vendor`. Edit at your peril; re-run the\n\
         # command instead. Source copying is not implemented yet, so\n\
         # declarations keep empty srcs placeholders.\n\
         #\n\
         # Crates with a build.rs (or that pull one through build-deps)\n\
         # are commented out; they need cargo.build_script wiring that is\n\
         # not yet plumbed end to end.\n\n",
    );

    let mut declared = 0usize;
    let mut skipped = 0usize;
    let mut skipped_names = Vec::new();

    let mut pkgs: Vec<&MetadataPackage> = metadata
        .packages
        .iter()
        .filter(|p| !workspace_ids.contains(&p.id))
        .filter(|p| {
            p.source
                .as_deref()
                .is_some_and(|s| s.starts_with("registry+"))
        })
        .collect();
    pkgs.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));

    for pkg in pkgs {
        let Some(lib_target) = pick_lib_target(pkg) else {
            skipped += 1;
            skipped_names.push(format!("{}-{} (no library target)", pkg.name, pkg.version));
            continue;
        };
        let crate_name = sanitize_crate_name(&pkg.name);
        let manifest_dir = manifest_dir_of(pkg)?;
        let src_root = relative_to(&manifest_dir, &lib_target.src_path)
            .unwrap_or_else(|| lib_target.src_path.clone());
        let dep_lines = deps_for(pkg, &normal_deps, &pkg_by_id);
        let is_proc_macro = lib_target.kind.iter().any(|k| k == "proc-macro");
        let edition = &lib_target.edition;

        if manifest_has_build_script(pkg) {
            write_skipped(
                &mut body,
                pkg,
                &crate_name,
                edition,
                &src_root,
                is_proc_macro,
                &dep_lines,
            );
            skipped += 1;
            skipped_names.push(format!("{}-{} (build.rs)", pkg.name, pkg.version));
        } else {
            write_declared(
                &mut body,
                pkg,
                &crate_name,
                edition,
                &src_root,
                is_proc_macro,
                &dep_lines,
            );
            declared += 1;
        }
    }

    Ok(VendorReport {
        manifest_body: body,
        declared,
        skipped,
        skipped_names,
    })
}

fn resolve_normal_deps(metadata: &CargoMetadata) -> BTreeMap<String, Vec<String>> {
    let Some(resolve) = &metadata.resolve else {
        return BTreeMap::new();
    };
    resolve
        .nodes
        .iter()
        .map(|node| {
            // We discard the dep's local rename (d.name) and only
            // record the pkg id; the consumer looks up the pkg by id.
            let edges: Vec<String> = node
                .deps
                .iter()
                .filter(|d| d.dep_kinds.is_empty() || d.dep_kinds.iter().any(|k| k.kind.is_none()))
                .map(|d| {
                    let _ = &d.name;
                    d.pkg.clone()
                })
                .collect();
            (node.id.clone(), edges)
        })
        .collect()
}

fn deps_for(
    pkg: &MetadataPackage,
    normal_deps: &BTreeMap<String, Vec<String>>,
    pkg_by_id: &BTreeMap<&String, &MetadataPackage>,
) -> Vec<String> {
    let Some(edges) = normal_deps.get(&pkg.id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dep_id in edges {
        if let Some(d) = pkg_by_id.get(dep_id) {
            if d.source
                .as_deref()
                .is_some_and(|s| s.starts_with("registry+"))
            {
                out.push(d.name.clone());
            }
        }
    }
    out
}

fn write_declared(
    out: &mut String,
    pkg: &MetadataPackage,
    crate_name: &str,
    edition: &str,
    src_root: &str,
    is_proc_macro: bool,
    deps: &[String],
) {
    let _ = writeln!(
        out,
        "# {} {} ({})",
        pkg.name, pkg.version, pkg.manifest_path
    );
    if is_proc_macro {
        out.push_str("[[rust.proc_macro]]\n");
    } else {
        out.push_str("[[rust.library]]\n");
    }
    let _ = writeln!(out, "name = {:?}", pkg.name);
    let _ = writeln!(out, "crate_name = {crate_name:?}");
    let _ = writeln!(out, "edition = {edition:?}");
    let _ = writeln!(out, "crate_root = {src_root:?}");
    out.push_str("srcs = [] # add src_globs once sources are copied into vendor/\n");
    let _ = writeln!(out, "deps = {}", toml_array(deps));
    out.push('\n');
}

fn write_skipped(
    out: &mut String,
    pkg: &MetadataPackage,
    crate_name: &str,
    edition: &str,
    src_root: &str,
    is_proc_macro: bool,
    deps: &[String],
) {
    let _ = writeln!(
        out,
        "# {} {}: needs build.rs ({})",
        pkg.name, pkg.version, pkg.manifest_path
    );
    if is_proc_macro {
        out.push_str("# [[rust.proc_macro]]\n");
    } else {
        out.push_str("# [[rust.library]]\n");
    }
    let _ = writeln!(out, "# name = {:?}", pkg.name);
    let _ = writeln!(out, "# crate_name = {crate_name:?}");
    let _ = writeln!(out, "# edition = {edition:?}");
    let _ = writeln!(out, "# crate_root = {src_root:?}");
    out.push_str("# srcs = [] # populate after vendoring sources\n");
    let _ = writeln!(out, "# deps = {}", toml_array(deps));
    out.push('\n');
}

fn pick_lib_target(pkg: &MetadataPackage) -> Option<&MetadataTarget> {
    pkg.targets.iter().find(|t| {
        t.kind
            .iter()
            .any(|k| k == "lib" || k == "rlib" || k == "proc-macro")
    })
}

fn manifest_has_build_script(pkg: &MetadataPackage) -> bool {
    pkg.targets
        .iter()
        .any(|t| t.kind.iter().any(|k| k == "custom-build"))
        || !pkg.build_dependencies.is_empty()
}

fn manifest_dir_of(pkg: &MetadataPackage) -> Result<String> {
    let path = PathBuf::from(&pkg.manifest_path);
    path.parent()
        .ok_or_else(|| anyhow!("manifest path has no parent: {}", pkg.manifest_path))
        .map(|p| p.to_string_lossy().into_owned())
}

fn relative_to(base: &str, full: &str) -> Option<String> {
    let base = Path::new(base);
    let full = Path::new(full);
    full.strip_prefix(base)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

fn sanitize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn toml_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        write!(out, "{value:?}").expect("writing to String cannot fail");
    }
    out.push(']');
    out
}
