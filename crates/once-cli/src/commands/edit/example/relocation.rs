use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};

use super::paths::{join_display_path, normalize_relative_path};

pub(super) fn relocate_manifest_references(
    files: &mut [once_frontend::TargetKindExampleFile],
    destination: &str,
) -> Result<()> {
    if destination.is_empty() {
        return Ok(());
    }
    let mut example_targets = BTreeSet::new();
    for file in files.iter() {
        if Path::new(&file.path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(once_frontend::TOML_BUILD_FILE_NAME)
        {
            continue;
        }
        let package = manifest_package(&file.path);
        let document: toml::Value = toml::from_str(&file.contents)
            .with_context(|| format!("parsing example manifest `{}`", file.path))?;
        for target in document
            .get("target")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = target
                .as_table()
                .and_then(|target| target.get("name"))
                .and_then(toml::Value::as_str)
            {
                example_targets.insert(once_frontend::target_id(&package, name));
            }
        }
    }

    for file in files.iter_mut() {
        if Path::new(&file.path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(once_frontend::TOML_BUILD_FILE_NAME)
        {
            continue;
        }
        let package = manifest_package(&file.path);
        let mut document: toml::Value = toml::from_str(&file.contents)
            .with_context(|| format!("parsing example manifest `{}`", file.path))?;
        for target in document
            .get_mut("target")
            .and_then(toml::Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let Some(target) = target.as_table_mut() else {
                continue;
            };
            if let Some(deps) = target.get_mut("deps") {
                relocate_dependency_value(deps, &package, destination, &example_targets)?;
            }
            if let Some(dependencies) = target
                .get_mut("dependencies")
                .and_then(toml::Value::as_table_mut)
            {
                for (_, deps) in dependencies.iter_mut() {
                    relocate_dependency_value(deps, &package, destination, &example_targets)?;
                }
            }
        }
        file.contents = toml::to_string_pretty(&document)
            .with_context(|| format!("rendering relocated example manifest `{}`", file.path))?;
    }
    Ok(())
}

fn relocate_dependency_value(
    value: &mut toml::Value,
    package: &str,
    destination: &str,
    example_targets: &BTreeSet<String>,
) -> Result<()> {
    let Some(dependencies) = value.as_array_mut() else {
        return Ok(());
    };
    for dependency in dependencies {
        let Some(raw) = dependency.as_str() else {
            continue;
        };
        let normalized = once_frontend::normalize_manifest_target(package, raw)
            .with_context(|| format!("normalizing example dependency `{raw}`"))?;
        if example_targets.contains(&normalized) {
            *dependency = toml::Value::String(join_display_path(destination, &normalized));
        }
    }
    Ok(())
}

fn manifest_package(path: &str) -> String {
    Path::new(path).parent().map_or_else(String::new, |parent| {
        normalize_relative_path(parent.to_string_lossy().as_ref())
    })
}
