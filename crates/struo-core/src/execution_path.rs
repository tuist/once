use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

pub const EXECUTION_ROOT_MARKER: &str = "{{struo.execution_root}}";
/// The executing user's home directory.
pub const HOME_MARKER: &str = "{{struo.home}}";
const DATA_DIR_MARKER: &str = "{{struo.data_dir}}";
const CACHE_DIR_MARKER: &str = "{{struo.cache_dir}}";

#[must_use]
pub fn resolve_execution_value(value: &str, execution_root: &Path) -> String {
    let mut value = value.replace(EXECUTION_ROOT_MARKER, &command_path(execution_root));
    if value.contains("{{struo.") {
        for (marker, path) in host_directories() {
            value = value.replace(marker, &command_path(&path));
        }
    }
    value
}

/// Spell paths inside Struo's own data and cache directories with markers
/// that resolve on the executing host, so action keys do not depend on where
/// the coordinator keeps them.
#[must_use]
pub fn portable_host_value(value: &str) -> String {
    let mut directories = host_directories();
    directories.retain(|(marker, _)| *marker != HOME_MARKER);
    directories.sort_by_key(|(_, path)| std::cmp::Reverse(path.as_os_str().len()));
    directories
        .into_iter()
        .fold(value.to_string(), |value, (marker, path)| {
            value.replace(&command_path(&path), marker)
        })
}

fn host_directories() -> Vec<(&'static str, std::path::PathBuf)> {
    let xdg = crate::Xdg::from_env();
    let mut directories = vec![
        (DATA_DIR_MARKER, xdg.struo_data()),
        (CACHE_DIR_MARKER, xdg.cache_home.join("struo")),
    ];
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(home) = std::env::var_os(home) {
        directories.push((HOME_MARKER, home.into()));
    }
    directories
}

/// Spell a path for a command line or environment value. On Windows,
/// `std::fs::canonicalize` returns verbatim paths (`\\?\D:\...`), which
/// shells such as Git Bash cannot open and which Windows rejects once a
/// command appends `/`-separated components. Those values get their plain
/// spelling; Struo's own filesystem calls keep the canonical path.
#[must_use]
pub fn command_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        strip_verbatim_prefix(&path).into_owned()
    } else {
        path.into_owned()
    }
}

/// Windows `MAX_PATH`: longer paths only resolve with the verbatim prefix.
const MAX_PATH: usize = 260;

fn strip_verbatim_prefix(path: &str) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        let plain = format!(r"\\{rest}");
        if plain.len() < MAX_PATH {
            return Cow::Owned(plain);
        }
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\';
        if drive && rest.len() < MAX_PATH {
            return Cow::Borrowed(rest);
        }
    }
    Cow::Borrowed(path)
}

#[must_use]
pub fn resolve_execution_argv(argv: &[String], execution_root: &Path) -> Vec<String> {
    argv.iter()
        .map(|value| resolve_execution_value(value, execution_root))
        .collect()
}

#[must_use]
pub fn resolve_execution_env(
    env: &BTreeMap<String, String>,
    execution_root: &Path,
) -> BTreeMap<String, String> {
    env.iter()
        .map(|(key, value)| (key.clone(), resolve_execution_value(value, execution_root)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_markers_without_changing_unmarked_values() {
        assert_eq!(
            resolve_execution_value(
                "{{struo.execution_root}}/.struo/cache",
                Path::new("/sandbox")
            ),
            "/sandbox/.struo/cache"
        );
        assert_eq!(
            resolve_execution_value("plain", Path::new("/sandbox")),
            "plain"
        );
    }

    #[test]
    fn host_directories_round_trip_through_markers() {
        let xdg = crate::Xdg::from_env();
        let data = command_path(&xdg.struo_data());
        let cache = command_path(&xdg.cache_home.join("struo"));
        let value = format!("{data}/tools/mise-data/installs/erlang:/usr/bin:{cache}/mise");
        let portable = portable_host_value(&value);
        assert_eq!(
            portable,
            "{{struo.data_dir}}/tools/mise-data/installs/erlang:/usr/bin:{{struo.cache_dir}}/mise"
        );
        assert_eq!(resolve_execution_value(&portable, Path::new("/ws")), value);
    }

    #[test]
    fn resolves_markers_inside_environment_lists() {
        assert_eq!(
            resolve_execution_value(
                "{{struo.execution_root}}/bin:/usr/bin",
                Path::new("/workspace")
            ),
            "/workspace/bin:/usr/bin"
        );
    }

    #[test]
    fn strips_verbatim_prefixes_that_have_a_plain_spelling() {
        assert_eq!(
            strip_verbatim_prefix(r"\\?\D:\a\_temp\source"),
            r"D:\a\_temp\source"
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\repo"),
            r"\\server\share\repo"
        );
        assert_eq!(strip_verbatim_prefix(r"D:\a\source"), r"D:\a\source");
        assert_eq!(strip_verbatim_prefix("/sandbox"), "/sandbox");
    }

    #[test]
    fn keeps_verbatim_prefixes_without_a_plain_spelling() {
        assert_eq!(
            strip_verbatim_prefix(r"\\?\Volume{0b1c}\repo"),
            r"\\?\Volume{0b1c}\repo"
        );
        let long = format!(r"\\?\D:\{}", "a".repeat(MAX_PATH));
        assert_eq!(strip_verbatim_prefix(&long), long);
        let long_unc = format!(r"\\?\UNC\server\{}", "a".repeat(MAX_PATH));
        assert_eq!(strip_verbatim_prefix(&long_unc), long_unc);
    }
}
