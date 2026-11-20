//! Host observations recorded during analysis, and the replay check that
//! decides whether a recorded analysis still holds.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use super::expansion::{pattern_matches, walk_package_entries};
use super::{
    cached_host_tree_digest, expand_globs_with_excludes, file_sha256_hex, host_arch_str,
    host_os_str, inherited_environment_sha256, read_bounded_host_file, sha256_hex,
};
use crate::analysis::store::{
    observe, with_store, AnalysisObservations, CommandPolicy, HostCache, Observation,
    HOST_COMMAND_OUTPUT_LIMIT,
};

/// Record where an unresolvable symlink pointed, so that a target appearing
/// later counts as a change.
///
/// Only meaningful for a link, and harmless for anything else: a path that
/// simply vanished mid-walk is observed as absent, which is what it is.
pub(super) fn observe_absent_link_target(path: &Path) {
    let target = match std::fs::read_link(path) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => match path.parent() {
            Some(parent) => parent.join(target),
            None => return,
        },
        Err(_) => path.to_path_buf(),
    };
    let _ = observe_host_path(&target);
}

pub(super) fn observe_host_path(path: &Path) -> Result<()> {
    with_store(|store| {
        let store = store.ok_or_else(|| anyhow!("host path observed outside analysis"))?;
        store.host_cache.observe_path(path);
        Ok(())
    })
}

pub(super) fn observe_platform() {
    observe(Observation::Platform {
        arch: host_arch_str().to_string(),
        os: host_os_str().to_string(),
    });
}

/// Resolve `name` on `PATH` and record the answer, including a miss.
///
/// A miss matters as much as a hit: a rule that took the optional branch
/// because a tool was absent has to be re-run once the tool appears, and
/// nothing else in the record would say so.
pub(super) fn observed_which(name: &str, caller: &str) -> Result<Option<String>> {
    let resolved = with_store(|store| -> Result<Option<String>> {
        let store = store.ok_or_else(|| anyhow!("{caller} called outside analysis"))?;
        store.host_cache.which(name)
    })?;
    observe(Observation::Which {
        name: name.to_string(),
        resolved: resolved.clone(),
    });
    Ok(resolved)
}

pub(super) fn read_host_dir_names(path: &str) -> Vec<String> {
    let mut names = match std::fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// Expand a set of patterns through the shared memo and record the answer.
///
/// `kind` selects the expansion rule and keeps two kinds of answer from being
/// mistaken for one another, both in the memo and in the record.
pub(super) fn observed_paths(
    kind: &'static str,
    patterns: &[String],
    excludes: &[String],
) -> Result<PathSet> {
    observed_paths_anchored(kind, None, patterns, excludes)
}

/// As [`observed_paths`], for an expansion anchored somewhere other than the
/// package of the target that ran it.
///
/// A walk over the whole workspace gives the same answer to every target, so it
/// is keyed and recorded at the root. Anchoring it under the calling package
/// instead would make one walk per package out of what is one question.
pub(super) fn observed_paths_anchored(
    kind: &'static str,
    anchor: Option<&str>,
    patterns: &[String],
    excludes: &[String],
) -> Result<PathSet> {
    let (package, resolved) = with_store(|store| -> Result<(String, PathSet)> {
        let store = store.ok_or_else(|| anyhow!("{kind} called outside analysis"))?;
        let package = anchor.unwrap_or(store.package.as_str());
        let resolved = store.host_cache.globs(
            kind,
            &store.workspace_root,
            package,
            patterns,
            excludes,
            || expand_observed_paths(kind, &store.workspace_root, package, patterns, excludes),
        )?;
        Ok((package.to_string(), resolved))
    })?;
    observe(Observation::Paths {
        expansion: kind.to_string(),
        package,
        patterns: patterns.to_vec(),
        excludes: excludes.to_vec(),
        matches: resolved.as_ref().clone(),
    });
    Ok(resolved)
}

type PathSet = Arc<Vec<String>>;

/// The expansion behind each path-returning global, in one place so recording
/// and replay cannot drift apart.
pub(super) fn expand_observed_paths(
    kind: &str,
    workspace_root: &Path,
    package: &str,
    patterns: &[String],
    excludes: &[String],
) -> Result<Vec<String>> {
    match kind {
        "glob" => expand_globs_with_excludes(workspace_root, package, patterns, excludes),
        "walk_files" | "walk_workspace_files" | "walk_symlinks" => {
            let root = patterns
                .first()
                .ok_or_else(|| anyhow!("{kind} requires a root"))?;
            let excluded_paths = excludes
                .iter()
                .filter_map(|entry| entry.strip_prefix("path:"))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let excluded_names = excludes
                .iter()
                .filter_map(|entry| entry.strip_prefix("name:"))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            walk_package_entries(
                workspace_root,
                package,
                root,
                &excluded_paths,
                &excluded_names,
                excludes.iter().any(|entry| entry == "empty-dirs"),
                kind == "walk_symlinks",
            )
        }
        other => Err(anyhow!("unknown path expansion `{other}`")),
    }
}

/// Whether every answer in `observations` is still the answer the host gives.
///
/// This is the whole of the replay decision. Anything it cannot check, it
/// reports as changed, so a record that mentions a channel this function does
/// not understand can never be reused.
pub(in crate::analysis) fn observations_hold(
    workspace_root: &Path,
    host_cache: &HostCache,
    observations: &AnalysisObservations,
    policy: CommandPolicy,
    unchanged: &UnchangedWorkspace,
) -> bool {
    if !observations.is_complete() {
        return false;
    }
    observations.entries().iter().all(|observation| {
        observation_holds(workspace_root, host_cache, observation, policy, unchanged)
    })
}

/// What a caller already knows about which workspace paths cannot have moved.
///
/// Re-expanding a path pattern is the most expensive kind of check here: it
/// walks a package to find out whether the same files are still there. A caller
/// holding a watcher's account of the window can skip that for any package the
/// watcher did not see a change under, which on a workspace of a few hundred
/// packages is the difference between one walk and hundreds.
pub enum UnchangedWorkspace {
    /// Nothing is known; every answer is checked by asking again.
    Unknown,
    /// Exactly these workspace-relative paths changed, and nothing else did.
    Except(std::collections::BTreeSet<String>),
}

impl UnchangedWorkspace {
    /// Whether re-expanding these patterns could produce a different answer.
    ///
    /// A changed path matters when the expansion would have selected it, which
    /// covers a file edited, created, or deleted: the watcher reports all three,
    /// and a pattern that matches the path is a pattern whose answer moved.
    ///
    /// Matching the patterns rather than the package is what makes this worth
    /// having. A graph derived from a package manifest anchors every target at
    /// the workspace root, so asking "did anything under the package change"
    /// answers yes for every target as soon as one file is edited, and every
    /// one of them re-walks.
    fn expansion_could_differ(&self, expansion: &str, package: &str, patterns: &[String]) -> bool {
        let Self::Except(changed) = self else {
            return true;
        };
        expansion_could_differ(changed, expansion, package, patterns)
    }
}

/// Whether an expansion selects whole subtrees rather than matching patterns.
///
/// A walk is handed a directory and returns what is under it, so anything
/// beneath that directory is part of its answer. Getting this wrong in the
/// quiet direction means a file added under a walked directory looks
/// irrelevant, and whatever read that walk is reused when it should not be.
pub(super) fn expansion_selects_subtrees(expansion: &str) -> bool {
    matches!(expansion, "walk_files" | "walk_workspace_files")
}

/// Whether re-running one recorded expansion could produce a different answer,
/// given everything that changed since it ran.
///
/// Shared so the two places that ask it, replaying an analysis and reusing a
/// target's outcome, cannot answer it differently.
pub fn expansion_could_differ(
    changed: &std::collections::BTreeSet<String>,
    expansion: &str,
    package: &str,
    patterns: &[String],
) -> bool {
    if changed.is_empty() {
        return false;
    }
    // A pattern that climbs out of its package is expanded against the whole
    // workspace, so there is no prefix that bounds what it can select.
    if patterns.iter().any(|pattern| pattern.contains("..")) {
        return true;
    }
    let prefix = if package.is_empty() {
        String::new()
    } else {
        format!("{package}/")
    };
    let compiled = (!expansion_selects_subtrees(expansion)).then(|| {
        patterns
            .iter()
            .filter_map(|pattern| {
                glob::Pattern::new(pattern.strip_prefix("./").unwrap_or(pattern)).ok()
            })
            .collect::<Vec<_>>()
    });
    changed.iter().any(|path| {
        let Some(relative) = path.strip_prefix(prefix.as_str()) else {
            return false;
        };
        match &compiled {
            Some(patterns) => patterns.iter().any(|p| pattern_matches(p, relative)),
            None => patterns.iter().any(|root| {
                let root = root.strip_prefix("./").unwrap_or(root);
                relative == root || relative.starts_with(&format!("{root}/"))
            }),
        }
    })
}

/// Name the first recorded answer that no longer matches, or `None` when they
/// all still do. Only for reporting: the decision is `observations_hold`.
pub(in crate::analysis) fn first_stale_observation(
    workspace_root: &Path,
    host_cache: &HostCache,
    observations: &AnalysisObservations,
    policy: CommandPolicy,
    unchanged: &UnchangedWorkspace,
) -> Option<String> {
    if !observations.is_complete() {
        return Some("analysis read something the ledger cannot describe".to_string());
    }
    observations
        .entries()
        .iter()
        .find(|observation| {
            !observation_holds(workspace_root, host_cache, observation, policy, unchanged)
        })
        .map(|observation| format!("{observation:?}"))
}

/// Replay a recorded answer about one host file's size or link destination.
pub(super) fn file_attribute_holds(observation: &Observation) -> bool {
    match observation {
        Observation::SymlinkTarget { path, target } => match std::fs::read_link(path) {
            Ok(actual) => actual.to_str().is_some_and(|actual| actual == target),
            Err(_) => target.is_empty(),
        },
        Observation::FileSize { path, bytes } => std::fs::metadata(path)
            .is_ok_and(|metadata| i64::try_from(metadata.len()).ok().as_ref() == Some(bytes)),
        Observation::FileExists { path, exists } => Path::new(path).is_file() == *exists,
        Observation::PathExists { path, exists } => Path::new(path).exists() == *exists,
        _ => false,
    }
}

pub(super) fn command_observation_holds(
    host_cache: &HostCache,
    observation: &Observation,
    policy: CommandPolicy,
) -> bool {
    let Observation::Command {
        argv,
        env,
        cwd,
        merge_stderr,
        check,
        output_sha256,
        program,
        max_output_bytes,
    } = observation
    else {
        return false;
    };
    match policy {
        // Ask the command again rather than reasoning about what it reads.
        // It is memoised for the whole invocation, so this costs one run
        // however many targets recorded it. A version probe answered from
        // the persisted tool cache is gated on the fingerprints of the tool
        // binaries themselves, so that answer is not older than the
        // toolchain it describes.
        CommandPolicy::Rerun => host_cache
            .command_checked(
                argv,
                env,
                cwd.as_deref().map(Path::new),
                *merge_stderr,
                *check,
                max_output_bytes.unwrap_or(HOST_COMMAND_OUTPUT_LIMIT),
            )
            .is_ok_and(|output| sha256_hex(output.as_bytes()) == *output_sha256),
        // The caller cannot afford to ask again, so the most it can
        // establish is that the same program would answer. A record written
        // before the program was described cannot establish even that.
        CommandPolicy::TrustDeclaredInputs => program.as_ref().is_some_and(|recorded| {
            argv.first()
                .and_then(|name| host_cache.program_identity(name))
                .is_some_and(|actual| &actual == recorded)
        }),
    }
}

pub(super) fn observation_holds(
    workspace_root: &Path,
    host_cache: &HostCache,
    observation: &Observation,
    policy: CommandPolicy,
    unchanged: &UnchangedWorkspace,
) -> bool {
    match observation {
        Observation::Platform { arch, os } => arch == host_arch_str() && os == host_os_str(),
        Observation::Env { name, value } => &host_cache.env(name) == value,
        Observation::EnvironmentDigest { sha256 } => &inherited_environment_sha256() == sha256,
        Observation::Which { name, resolved } => {
            host_cache.which(name).ok().as_ref() == Some(resolved)
        }
        Observation::Command { .. } => command_observation_holds(host_cache, observation, policy),
        Observation::SymlinkTarget { .. }
        | Observation::FileSize { .. }
        | Observation::FileExists { .. }
        | Observation::PathExists { .. } => file_attribute_holds(observation),
        Observation::FileDigest { path, sha256 } => host_cache
            .host_file_digest(Path::new(path), || file_sha256_hex(Path::new(path)))
            .is_ok_and(|actual| &actual == sha256),
        Observation::FileMode { path, mode } => std::fs::metadata(path)
            .is_ok_and(|metadata| struo_host_tree::host_file_mode(&metadata) == *mode),
        Observation::TreeDigest { path, sha256 } => {
            let source = Path::new(path);
            source.is_dir()
                && cached_host_tree_digest(host_cache, workspace_root, source)
                    .is_ok_and(|actual| &actual == sha256)
        }
        Observation::PathWithin { path, root, within } => {
            let resolved = |value: &str| std::fs::canonicalize(Path::new(value)).ok();
            match (resolved(path), resolved(root)) {
                (Some(path), Some(root)) => path.starts_with(root) == *within,
                // One of them no longer resolves, so the question the record
                // answered is not the question being asked now.
                _ => false,
            }
        }
        Observation::FileText { path, sha256 } => {
            read_bounded_host_file(path).is_ok_and(|bytes| &sha256_hex(&bytes) == sha256)
        }
        Observation::FileContains {
            path,
            needle,
            found,
        } => {
            let actual = if needle.is_empty() {
                true
            } else {
                read_bounded_host_file(path).is_ok_and(|content| {
                    content
                        .windows(needle.len())
                        .any(|window| window == needle.as_bytes())
                })
            };
            actual == *found
        }
        Observation::ReadDir { path, names } => {
            host_cache.observe_path(Path::new(path));
            &read_host_dir_names(path) == names
        }
        Observation::Paths {
            expansion,
            package,
            patterns,
            excludes,
            matches,
        } => {
            if !unchanged.expansion_could_differ(expansion, package, patterns) {
                return true;
            }
            host_cache
                .globs(
                    path_expansion_kind(expansion),
                    workspace_root,
                    package,
                    patterns,
                    excludes,
                    || {
                        expand_observed_paths(
                            expansion,
                            workspace_root,
                            package,
                            patterns,
                            excludes,
                        )
                    },
                )
                .is_ok_and(|actual| actual.as_ref() == matches)
        }
    }
}

/// Map a recorded expansion name back to the `'static` key the memo uses.
///
/// An unrecognised name resolves to a key no expansion answers to, so the
/// replay fails rather than quietly reading another kind's answer.
pub(super) fn path_expansion_kind(kind: &str) -> &'static str {
    match kind {
        "glob" => "glob",
        "walk_files" => "walk_files",
        "walk_workspace_files" => "walk_workspace_files",
        "walk_symlinks" => "walk_symlinks",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::UnchangedWorkspace;

    fn since(paths: &[&str]) -> UnchangedWorkspace {
        UnchangedWorkspace::Except(
            paths
                .iter()
                .map(|p| (*p).to_string())
                .collect::<BTreeSet<_>>(),
        )
    }

    fn patterns(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// The case this exists for. A graph derived from a package manifest gives
    /// every target the workspace root as its package, so a package-level test
    /// would call every expansion in the workspace suspect for one edit.
    #[test]
    fn an_edit_elsewhere_leaves_a_root_anchored_pattern_alone() {
        let changed = since(&["crates/core/main.rs"]);

        assert!(!changed.expansion_could_differ("glob", "", &patterns(&["crates/ignore/*.rs"])));
        assert!(changed.expansion_could_differ("glob", "", &patterns(&["crates/core/*.rs"])));
    }

    #[test]
    fn a_file_that_appears_under_a_matching_pattern_is_a_difference() {
        let changed = since(&["src/added.rs"]);

        assert!(changed.expansion_could_differ("glob", "", &patterns(&["src/**/*.rs"])));
    }

    #[test]
    fn a_pattern_is_matched_relative_to_its_package() {
        let changed = since(&["apps/tool/src/lib.rs"]);

        assert!(changed.expansion_could_differ("glob", "apps/tool", &patterns(&["src/*.rs"])));
        assert!(!changed.expansion_could_differ("glob", "apps/other", &patterns(&["src/*.rs"])));
    }

    /// A walk is given a directory, not a pattern, so everything beneath it is
    /// part of the answer.
    #[test]
    fn a_walk_covers_everything_beneath_its_root() {
        let changed = since(&["assets/images/logo.png"]);

        assert!(changed.expansion_could_differ("walk_files", "", &patterns(&["assets"])));
        assert!(!changed.expansion_could_differ("walk_files", "", &patterns(&["docs"])));
    }

    /// Every kind of walk owns its subtree, not just the package-relative one.
    /// Treating a workspace walk's directory as a glob pattern would match the
    /// directory itself and nothing under it, so a header dropped into a
    /// searched directory would look irrelevant.
    #[test]
    fn a_workspace_walk_covers_everything_beneath_its_root() {
        let changed = since(&["apps/Hello/Sources/Extra.h"]);

        assert!(changed.expansion_could_differ(
            "walk_workspace_files",
            "",
            &patterns(&["apps/Hello/Sources"])
        ));
        assert!(!changed.expansion_could_differ(
            "walk_workspace_files",
            "",
            &patterns(&["apps/Other/Sources"])
        ));
    }

    /// A pattern that climbs out of its package is expanded against the whole
    /// workspace, so nothing bounds what it can select.
    #[test]
    fn a_pattern_leaving_its_package_is_always_suspect() {
        let changed = since(&["somewhere/else.rs"]);

        assert!(changed.expansion_could_differ(
            "glob",
            "apps/tool",
            &patterns(&["../shared/*.rs"])
        ));
    }

    #[test]
    fn nothing_changed_leaves_every_expansion_alone() {
        assert!(!since(&[]).expansion_could_differ("glob", "", &patterns(&["**/*.rs"])));
    }

    /// With no watcher there is nothing to reason from, so every expansion is
    /// re-run.
    #[test]
    fn an_unknown_window_settles_nothing() {
        assert!(UnchangedWorkspace::Unknown.expansion_could_differ("glob", "", &patterns(&["a"])));
    }
}
