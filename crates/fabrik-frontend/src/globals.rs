//! Built-in Starlark globals exposed to `fabrik.star` files.
//!
//! Two primitives:
//! - `target(kind, name, srcs, deps)` records a target. Higher-level
//!   typed wrappers (`rust_binary`, `rust_library`, `rust_test`, etc.)
//!   are defined in Starlark and live in [`crate::prelude`], not here.
//! - `glob(patterns)` expands shell-style patterns relative to the
//!   current package directory.

use std::collections::BTreeMap;

use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::Value;

use crate::eval::with_state;
use crate::target::Target;

pub(crate) fn unpack_str_list(value: Option<Value<'_>>) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list =
        ListRef::from_value(value).ok_or_else(|| anyhow::anyhow!("expected a list of strings"))?;
    list.iter()
        .map(|item| {
            item.unpack_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("list element is not a string"))
        })
        .collect()
}

fn unpack_str_dict(value: Option<Value<'_>>) -> anyhow::Result<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let dict =
        DictRef::from_value(value).ok_or_else(|| anyhow::anyhow!("expected a dict of strings"))?;
    dict.iter()
        .map(|(key, value)| {
            let key = key
                .unpack_str()
                .ok_or_else(|| anyhow::anyhow!("dict key is not a string"))?;
            let value = value
                .unpack_str()
                .ok_or_else(|| anyhow::anyhow!("dict value is not a string"))?;
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

fn record_target(
    kind: &str,
    name: &str,
    srcs: Vec<String>,
    deps: Vec<String>,
    attrs: BTreeMap<String, String>,
) {
    with_state(|s| {
        s.targets.push(Target {
            package: s.package.clone(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            srcs,
            deps,
            attrs,
        });
    });
}

fn glob_expand(patterns: &[String]) -> anyhow::Result<Vec<String>> {
    with_state(|s| {
        let pkg_dir = s.workspace_root.join(&s.package);
        let strip_prefix = pkg_dir.clone();
        let mut out = Vec::new();
        for pattern in patterns {
            let abs_pattern = pkg_dir.join(pattern);
            let pattern_str = abs_pattern.to_str().ok_or_else(|| {
                anyhow::anyhow!("non-utf8 glob pattern: {}", abs_pattern.display())
            })?;
            for entry in glob::glob(pattern_str)
                .map_err(|e| anyhow::anyhow!("invalid glob pattern `{pattern}`: {e}"))?
            {
                let path =
                    entry.map_err(|e| anyhow::anyhow!("glob walk failed for `{pattern}`: {e}"))?;
                if !path.is_file() {
                    continue;
                }
                let rel = path
                    .strip_prefix(&strip_prefix)
                    .map_err(|_| anyhow::anyhow!("glob match outside package: {}", path.display()))?
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                out.push(rel);
            }
        }
        // Stable order: glob iterates in filesystem order, which varies.
        out.sort();
        out.dedup();
        Ok(out)
    })
}

#[starlark_module]
pub(crate) fn fabrik_globals(builder: &mut GlobalsBuilder) {
    /// Record a target. Called by the bundled prelude's typed wrappers
    /// (`rust_binary`, `rust_library`, `rust_test`, etc.) to push the
    /// target onto the current evaluation's target list. Plugin authors
    /// reach the same primitive when defining their own target types.
    fn target<'v>(
        #[starlark(require = named)] kind: &str,
        #[starlark(require = named)] name: &str,
        #[starlark(require = named)] srcs: Option<Value<'v>>,
        #[starlark(require = named)] deps: Option<Value<'v>>,
        #[starlark(require = named)] attrs: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        record_target(
            kind,
            name,
            unpack_str_list(srcs)?,
            unpack_str_list(deps)?,
            unpack_str_dict(attrs)?,
        );
        Ok(NoneType)
    }

    /// Expand one or more shell-style glob patterns relative to the
    /// current package directory and return the matching file paths
    /// (package-relative, sorted, deduplicated).
    fn glob<'v>(#[starlark(require = pos)] patterns: Value<'v>) -> anyhow::Result<Vec<String>> {
        let patterns = unpack_str_list(Some(patterns))?;
        glob_expand(&patterns)
    }
}
