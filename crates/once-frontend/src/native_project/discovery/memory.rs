use std::mem::size_of;
use std::path::Path;

use crate::error::{Error, Result};

use super::super::{NativeProjectMatch, NativeProjectSchema};

pub(super) const MAX_RETAINED_MATCH_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct RetainedMatches {
    matches: Vec<NativeProjectMatch>,
    stop_roots: Vec<Vec<String>>,
    rooted_stop_schemas: usize,
    retained_bytes: usize,
    limit_bytes: usize,
}

impl RetainedMatches {
    pub(super) fn new(root: &Path, schema_count: usize, limit_bytes: usize) -> Result<Self> {
        let projected_schema_bytes = schema_count.saturating_mul(size_of::<Vec<String>>());
        if projected_schema_bytes > limit_bytes {
            return Err(discovery_memory_error(root, limit_bytes));
        }
        let mut stop_roots = Vec::new();
        stop_roots
            .try_reserve_exact(schema_count)
            .map_err(|_| discovery_memory_error(root, limit_bytes))?;
        stop_roots.resize_with(schema_count, Vec::new);
        let retained_bytes = stop_roots
            .capacity()
            .saturating_mul(size_of::<Vec<String>>());
        if retained_bytes > limit_bytes {
            return Err(discovery_memory_error(root, limit_bytes));
        }
        Ok(Self {
            matches: Vec::new(),
            stop_roots,
            rooted_stop_schemas: 0,
            retained_bytes,
            limit_bytes,
        })
    }

    pub(super) fn insert(
        &mut self,
        root: &Path,
        schema_index: usize,
        schema: &NativeProjectSchema,
        package: &str,
    ) -> Result<()> {
        if schema.on_match == "stop" {
            if self.stop_roots[schema_index]
                .iter()
                .any(|root| package_is_within(package, root))
            {
                return Ok(());
            }
            let mut removed_bytes = 0usize;
            self.stop_roots[schema_index].retain(|root| {
                if package_is_within(root, package) {
                    removed_bytes = removed_bytes.saturating_add(root.capacity());
                    false
                } else {
                    true
                }
            });
            self.matches.retain(|matched| {
                if matched.native_project == schema.name
                    && package_is_within(&matched.package, package)
                {
                    removed_bytes = removed_bytes.saturating_add(owned_match_bytes(matched));
                    false
                } else {
                    true
                }
            });
            self.retained_bytes = self.retained_bytes.saturating_sub(removed_bytes);
        }

        let stop_root_bytes = if schema.on_match == "stop" {
            package.len()
        } else {
            0
        };
        let projected_bytes =
            projected_owned_match_bytes(schema, package).saturating_add(stop_root_bytes);
        if projected_bytes > self.limit_bytes.saturating_sub(self.retained_bytes) {
            return Err(discovery_memory_error(root, self.limit_bytes));
        }
        let available_capacity_bytes = self
            .limit_bytes
            .saturating_sub(self.retained_bytes)
            .saturating_sub(projected_bytes);
        let mut capacity_bytes = reserve_one(
            root,
            &mut self.matches,
            available_capacity_bytes,
            self.limit_bytes,
        )?;
        if schema.on_match == "stop" {
            capacity_bytes = capacity_bytes.saturating_add(reserve_one(
                root,
                &mut self.stop_roots[schema_index],
                available_capacity_bytes.saturating_sub(capacity_bytes),
                self.limit_bytes,
            )?);
        }
        if projected_bytes.saturating_add(capacity_bytes)
            > self.limit_bytes.saturating_sub(self.retained_bytes)
        {
            return Err(discovery_memory_error(root, self.limit_bytes));
        }

        let seed_target = crate::target_ref::target_id(package, &schema.target_name);
        let matched = NativeProjectMatch {
            native_project: schema.name.clone(),
            package: package.to_string(),
            markers: schema.markers.clone(),
            seed_target,
        };
        let stop_root = (schema.on_match == "stop").then(|| package.to_string());
        let owned_bytes = owned_match_bytes(&matched)
            .saturating_add(stop_root.as_ref().map_or(0, String::capacity));
        if owned_bytes.saturating_add(capacity_bytes)
            > self.limit_bytes.saturating_sub(self.retained_bytes)
        {
            return Err(discovery_memory_error(root, self.limit_bytes));
        }
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(owned_bytes)
            .saturating_add(capacity_bytes);
        self.matches.push(matched);
        if let Some(stop_root) = stop_root {
            if stop_root.is_empty() {
                self.rooted_stop_schemas = self.rooted_stop_schemas.saturating_add(1);
            }
            self.stop_roots[schema_index].push(stop_root);
        }
        Ok(())
    }

    pub(super) fn all_stop_schemas_rooted(&self, stop_schema_count: usize) -> bool {
        self.rooted_stop_schemas == stop_schema_count
    }

    pub(super) fn stop_covers(&self, schema_index: usize, root: &Path, package: &Path) -> bool {
        let relative = package.strip_prefix(root).unwrap_or(package);
        self.stop_roots[schema_index]
            .iter()
            .any(|stop_root| stop_root.is_empty() || relative.starts_with(Path::new(stop_root)))
    }

    pub(super) fn finish(self) -> Vec<NativeProjectMatch> {
        self.matches
    }
}

fn reserve_one<T>(
    root: &Path,
    values: &mut Vec<T>,
    available_bytes: usize,
    limit_bytes: usize,
) -> Result<usize> {
    if values.len() < values.capacity() {
        return Ok(0);
    }
    let slot_bytes = size_of::<T>().max(1);
    let maximum_growth = available_bytes / slot_bytes;
    if maximum_growth == 0 {
        return Err(discovery_memory_error(root, limit_bytes));
    }
    let growth = values.capacity().max(1).min(maximum_growth);
    let previous_capacity = values.capacity();
    values
        .try_reserve_exact(growth)
        .map_err(|_| discovery_memory_error(root, limit_bytes))?;
    let allocated_bytes = values
        .capacity()
        .saturating_sub(previous_capacity)
        .saturating_mul(slot_bytes);
    if allocated_bytes > available_bytes {
        return Err(discovery_memory_error(root, limit_bytes));
    }
    Ok(allocated_bytes)
}

fn projected_owned_match_bytes(schema: &NativeProjectSchema, package: &str) -> usize {
    let seed_target_bytes = if package.is_empty() {
        schema.target_name.len()
    } else {
        package
            .len()
            .saturating_add(1)
            .saturating_add(schema.target_name.len())
    };
    schema
        .markers
        .iter()
        .fold(0usize, |bytes, marker| bytes.saturating_add(marker.len()))
        .saturating_add(schema.markers.len().saturating_mul(size_of::<String>()))
        .saturating_add(schema.name.len())
        .saturating_add(package.len())
        .saturating_add(seed_target_bytes)
}

fn owned_match_bytes(matched: &NativeProjectMatch) -> usize {
    matched
        .markers
        .iter()
        .fold(0usize, |bytes, marker| {
            bytes.saturating_add(marker.capacity())
        })
        .saturating_add(
            matched
                .markers
                .capacity()
                .saturating_mul(size_of::<String>()),
        )
        .saturating_add(matched.native_project.capacity())
        .saturating_add(matched.package.capacity())
        .saturating_add(matched.seed_target.capacity())
}

fn discovery_memory_error(root: &Path, limit_bytes: usize) -> Error {
    Error::Eval {
        path: root.display().to_string(),
        message: format!(
            "native project discovery exceeded its {limit_bytes} byte retained-result limit; narrow workspace includes or excludes, or use `on_match = \"stop\"` when nested projects belong to one workspace"
        ),
    }
}

fn package_is_within(package: &str, root: &str) -> bool {
    package == root
        || root.is_empty()
        || package
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> NativeProjectSchema {
        NativeProjectSchema {
            name: "native".to_string(),
            docs: String::new(),
            markers: vec!["native.project".to_string()],
            target_name: "native".to_string(),
            target_kind: "native_workspace".to_string(),
            inputs: Vec::new(),
            exclude: Vec::new(),
            input_exclude: Vec::new(),
            on_match: "stop".to_string(),
            max_depth: 16,
            requires_tools: Vec::new(),
        }
    }

    #[test]
    fn stop_releases_descendant_matches_if_a_parent_arrives_later() {
        let temporary = tempfile::tempdir().unwrap();
        let native = schema();
        let mut retained =
            RetainedMatches::new(temporary.path(), 1, MAX_RETAINED_MATCH_BYTES).unwrap();

        retained
            .insert(temporary.path(), 0, &native, "workspace/a")
            .unwrap();
        retained
            .insert(temporary.path(), 0, &native, "workspace/b")
            .unwrap();
        let nested_bytes = retained.retained_bytes;
        retained
            .insert(temporary.path(), 0, &native, "workspace")
            .unwrap();

        assert_eq!(retained.matches.len(), 1);
        assert_eq!(retained.matches[0].package, "workspace");
        assert!(retained.retained_bytes < nested_bytes);
    }
}
