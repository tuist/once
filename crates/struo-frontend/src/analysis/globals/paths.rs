//! Path expansion over the active package and the workspace.

use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Value;

use super::observation::{observed_paths, observed_paths_anchored};
use crate::analysis::store::analysis_active;
use crate::analysis::values::unpack_string_list;

#[starlark_module]
pub(super) fn path_globals(builder: &mut GlobalsBuilder) {
    /// Expand a list of glob patterns against the active target's
    /// package directory. Returns sorted, deduplicated, workspace-
    /// relative file paths. Schema parsing returns an empty list.
    fn glob<'v>(
        patterns: Value<'v>,
        exclude: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let patterns = unpack_string_list(patterns, "patterns")?;
        let excludes = exclude
            .map(|value| unpack_string_list(value, "exclude"))
            .transpose()?
            .unwrap_or_default();
        let resolved = observed_paths("glob", &patterns, &excludes)?;
        Ok(heap.alloc(resolved.as_ref().clone()))
    }

    /// Walk a package-relative directory and return sorted,
    /// deduplicated, workspace-relative file and symbolic-link paths.
    /// `excluded_paths` names root-relative paths whose trees should
    /// not be traversed. `excluded_names` prunes entries with an exact
    /// file name at any depth. Schema parsing returns an empty list.
    fn walk_files<'v>(
        root: &str,
        excluded_paths: Option<Value<'v>>,
        excluded_names: Option<Value<'v>>,
        include_empty_directories: Option<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let excluded_paths = excluded_paths
            .map(|value| unpack_string_list(value, "excluded_paths"))
            .transpose()?
            .unwrap_or_default();
        let excluded_names = excluded_names
            .map(|value| unpack_string_list(value, "excluded_names"))
            .transpose()?
            .unwrap_or_default();
        // Memoised like `glob`: a target that walks the same tree for several
        // action input sets pays one walk, and the recorded answer is what a
        // later validation compares against. The two exclusion kinds are
        // tagged into one list so the memo key and the recorded answer keep
        // them apart.
        let excludes = excluded_paths
            .iter()
            .map(|path| format!("path:{path}"))
            .chain(excluded_names.iter().map(|name| format!("name:{name}")))
            .chain(
                include_empty_directories
                    .unwrap_or(false)
                    .then(|| "empty-dirs".to_string()),
            )
            .collect::<Vec<_>>();
        let resolved = observed_paths("walk_files", &[root.to_string()], &excludes)?;
        Ok(heap.alloc(resolved.as_ref().clone()))
    }

    /// Walk a package-relative directory and return one `path<NUL>target` string
    /// for every symbolic link under it, without following any. A target kind
    /// uses it to declare the files a linked input depends on with one walk
    /// and one recorded observation.
    fn walk_symlinks<'v>(
        root: &str,
        excluded_paths: Option<Value<'v>>,
        excluded_names: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let excluded_paths = excluded_paths
            .map(|value| unpack_string_list(value, "excluded_paths"))
            .transpose()?
            .unwrap_or_default();
        let excluded_names = excluded_names
            .map(|value| unpack_string_list(value, "excluded_names"))
            .transpose()?
            .unwrap_or_default();
        let excludes = excluded_paths
            .iter()
            .map(|path| format!("path:{path}"))
            .chain(excluded_names.iter().map(|name| format!("name:{name}")))
            .collect::<Vec<_>>();
        let resolved = observed_paths("walk_symlinks", &[root.to_string()], &excludes)?;
        Ok(heap.alloc(resolved.as_ref().clone()))
    }

    /// Walk a workspace-relative directory and return sorted, deduplicated,
    /// workspace-relative file and symbolic-link paths. Unlike `glob`, this
    /// does not follow symbolic links, so callers can inspect and explicitly
    /// materialize links whose destinations live outside the workspace.
    fn walk_workspace_files<'v>(
        root: &str,
        excluded_paths: Option<Value<'v>>,
        excluded_names: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let excluded_paths = excluded_paths
            .map(|value| unpack_string_list(value, "excluded_paths"))
            .transpose()?
            .unwrap_or_default();
        let excluded_names = excluded_names
            .map(|value| unpack_string_list(value, "excluded_names"))
            .transpose()?
            .unwrap_or_default();
        // Recorded and memoised like the other walks, and anchored at the
        // workspace root, which is what this one walks whichever target asks.
        let excludes = excluded_paths
            .iter()
            .map(|path| format!("path:{path}"))
            .chain(excluded_names.iter().map(|name| format!("name:{name}")))
            .collect::<Vec<_>>();
        let resolved = observed_paths_anchored(
            "walk_workspace_files",
            Some(""),
            &[root.to_string()],
            &excludes,
        )?;
        Ok(heap.alloc(resolved.as_ref().clone()))
    }
}
