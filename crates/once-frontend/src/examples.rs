//! Rule example bundles. Each example is a real on-disk workspace
//! under `prelude/examples/<slug>/`; we bake the tree into the binary
//! with `include_dir!` so MCP and CLI consumers get the same files a
//! human browsing the repo would see.
//!
//! Each example directory contains a `_meta.toml` (`name`, `use_when`)
//! plus the files that make up the runnable workspace. The meta file
//! is metadata only and never appears in the file bundle returned to
//! callers.

use include_dir::{include_dir, Dir};
use serde::Deserialize;

use crate::graph::{RuleExample, RuleExampleFile};

static EXAMPLES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/prelude/examples");

const META_FILE_NAME: &str = "_meta.toml";

#[derive(Debug, Deserialize)]
struct ExampleMeta {
    name: String,
    use_when: String,
}

/// Load a single example by slug. Returns `None` if no example with
/// that slug is bundled, or if the example's `_meta.toml` is missing
/// or malformed.
#[must_use]
pub fn load_example(slug: &str) -> Option<RuleExample> {
    let dir = EXAMPLES_DIR.get_dir(slug)?;
    let meta_path = format!("{slug}/{META_FILE_NAME}");
    let meta_file = EXAMPLES_DIR.get_file(&meta_path)?;
    let raw_meta = meta_file.contents_utf8()?;
    let meta: ExampleMeta = toml::from_str(raw_meta).ok()?;

    let mut files = Vec::new();
    collect_files(dir, slug, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Some(RuleExample {
        name: meta.name,
        slug: slug.to_string(),
        use_when: meta.use_when,
        files,
    })
}

/// List every example slug bundled in the binary, sorted.
#[must_use]
pub fn list_example_slugs() -> Vec<String> {
    let mut slugs: Vec<String> = EXAMPLES_DIR
        .dirs()
        .filter_map(|dir| dir.path().file_name()?.to_str().map(str::to_string))
        .collect();
    slugs.sort();
    slugs
}

fn collect_files(dir: &Dir<'_>, slug: &str, out: &mut Vec<RuleExampleFile>) {
    for file in dir.files() {
        let path = file.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(META_FILE_NAME) {
            continue;
        }
        let relative = path.strip_prefix(slug).unwrap_or(path);
        let contents = file.contents_utf8().unwrap_or_default().to_string();
        out.push(RuleExampleFile {
            path: relative.to_string_lossy().to_string(),
            contents,
        });
    }
    for sub in dir.dirs() {
        collect_files(sub, slug, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_example_returns_apple_library_minimal() {
        let example = load_example("apple-library-minimal").expect("example loads");
        assert_eq!(example.slug, "apple-library-minimal");
        assert!(!example.name.is_empty());
        assert!(!example.use_when.is_empty());
        // The example must include at least the manifest and one source file.
        assert!(example
            .files
            .iter()
            .any(|f| f.path == "apps/Hello/once.toml"));
        assert!(example
            .files
            .iter()
            .any(|f| f.path == "apps/Hello/Sources/Hello.swift"));
        // _meta.toml must never leak into the file bundle.
        assert!(!example.files.iter().any(|f| f.path.ends_with("_meta.toml")));
    }

    #[test]
    fn load_example_returns_none_for_unknown_slug() {
        assert!(load_example("does-not-exist").is_none());
    }

    #[test]
    fn list_example_slugs_returns_every_bundled_example() {
        let slugs = list_example_slugs();
        assert!(slugs.contains(&"apple-library-minimal".to_string()));
        assert!(slugs.contains(&"apple-library-with-objc".to_string()));
        assert!(slugs.contains(&"apple-application-minimal".to_string()));
        assert!(slugs.contains(&"rust-library-minimal".to_string()));
        assert!(slugs.contains(&"rust-binary-with-crate".to_string()));
    }

    #[test]
    fn collect_files_sorts_paths_deterministically() {
        let example = load_example("apple-library-with-objc").expect("example loads");
        let paths: Vec<&str> = example.files.iter().map(|f| f.path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted);
    }
}
