//! Built-in Starlark globals exposed to `fabrik.star` files.
//!
//! Today: three target builders (`rust_binary`, `rust_library`,
//! `rust_test`) plus a `glob` primitive. Target builders push into the
//! per-thread [`crate::eval::EvalState`]; `glob` reads the package
//! directory from it and walks the filesystem.

use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
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

fn record_target(kind: &str, name: &str, srcs: Vec<String>, deps: Vec<String>) {
    with_state(|s| {
        s.targets.push(Target {
            package: s.package.clone(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            srcs,
            deps,
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
    fn rust_binary<'v>(
        #[starlark(require = named)] name: &str,
        #[starlark(require = named)] srcs: Option<Value<'v>>,
        #[starlark(require = named)] deps: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        record_target(
            "rust_binary",
            name,
            unpack_str_list(srcs)?,
            unpack_str_list(deps)?,
        );
        Ok(NoneType)
    }

    fn rust_library<'v>(
        #[starlark(require = named)] name: &str,
        #[starlark(require = named)] srcs: Option<Value<'v>>,
        #[starlark(require = named)] deps: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        record_target(
            "rust_library",
            name,
            unpack_str_list(srcs)?,
            unpack_str_list(deps)?,
        );
        Ok(NoneType)
    }

    fn rust_test<'v>(
        #[starlark(require = named)] name: &str,
        #[starlark(require = named)] srcs: Option<Value<'v>>,
        #[starlark(require = named)] deps: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        record_target(
            "rust_test",
            name,
            unpack_str_list(srcs)?,
            unpack_str_list(deps)?,
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
