//! Detection of CI environments and the persistent cache volumes some
//! runners expose.
//!
//! Two questions are answered independently so neither is hard-wired to a
//! single vendor:
//!
//! - *Are we on a recognised CI provider?* Decided by [`CI_MARKERS`].
//! - *Does a runner mount a persistent cache volume, and where?* Decided
//!   by [`CACHE_VOLUME_RUNNERS`].
//!
//! Both are data-driven tables. Supporting another CI provider (Bitrise,
//! `CircleCI`, ...) or another runner provider (Namespace, ...) is a matter
//! of adding a row, not of threading new boolean flags through the
//! detection logic.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Environment variables whose presence marks a supported CI provider.
///
/// A cache volume is only trusted when one of these is set, so that a
/// stray `NSC_*`/`NSCLOUD_*` variable on a developer machine does not
/// redirect the local cache.
const CI_MARKERS: &[&str] = &[
    // GitHub Actions
    "GITHUB_ACTIONS",
    // Depot CI runs GitHub Actions workflows and documents compatibility
    // with standard GitHub Actions runner environment variables, so it is
    // expected to surface `GITHUB_ACTIONS` rather than a Depot-specific
    // marker here.
    //
    // GitLab CI/CD
    "GITLAB_CI",
    // Semaphore
    "SEMAPHORE",
    // Jenkins
    "JENKINS_URL",
];

/// A runner provider that mounts a persistent cache volume into the build.
struct CacheVolumeRunner {
    /// Recognises the runner from the name of an environment variable.
    signals: fn(&str) -> bool,
    /// Path where the runner mounts its cache volume.
    mount: &'static str,
}

/// Known runners that expose a persistent cache volume.
const CACHE_VOLUME_RUNNERS: &[CacheVolumeRunner] = &[
    // Namespace (namespace.so) cloud runners expose a cache volume at
    // `/cache` and advertise themselves through `NSC_*` / `NSCLOUD_*`.
    CacheVolumeRunner {
        signals: |key| key.starts_with("NSC_") || key.starts_with("NSCLOUD_"),
        mount: "/cache",
    },
];

/// Mount of a CI-provided persistent cache volume, if the process runs on
/// a recognised CI runner that exposes one and the mount is present.
///
/// Returns the bare mount point; callers decide which subtree they own.
pub(crate) fn cache_volume_mount() -> Option<PathBuf> {
    detect_cache_volume_mount(None, env::vars_os())
}

/// Pure core of [`cache_volume_mount`], exposed so tests can drive it with
/// an arbitrary variable set and a stand-in mount point.
///
/// `mount_override` replaces the matched runner's real mount (e.g. `/cache`)
/// with a path the test controls.
fn detect_cache_volume_mount<I, K, V>(mount_override: Option<&Path>, vars: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut on_ci = false;
    let mut runner: Option<&CacheVolumeRunner> = None;
    for (key, value) in vars {
        if value.as_ref().is_empty() {
            continue;
        }
        let key = key.as_ref().to_string_lossy();
        on_ci |= CI_MARKERS.contains(&key.as_ref());
        if runner.is_none() {
            runner = CACHE_VOLUME_RUNNERS.iter().find(|runner| (runner.signals)(&key));
        }
    }

    if !on_ci {
        return None;
    }

    let runner = runner?;
    let mount = mount_override.unwrap_or_else(|| Path::new(runner.mount));
    mount_is_dir(mount).then(|| mount.to_path_buf())
}

fn mount_is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_mount_for_ci_runner_with_present_volume() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cache-volume");
        std::fs::create_dir(&mount).unwrap();

        assert_eq!(
            detect_cache_volume_mount(
                Some(&mount),
                [("GITHUB_ACTIONS", "true"), ("NSC_WORKSPACE", "workspace")],
            ),
            Some(mount.clone())
        );
    }

    #[test]
    fn ignores_runner_without_recognised_ci_provider() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cache-volume");
        std::fs::create_dir(&mount).unwrap();

        assert_eq!(
            detect_cache_volume_mount(Some(&mount), [("NSC_WORKSPACE", "workspace")]),
            None
        );
    }

    #[test]
    fn ignores_ci_provider_without_cache_volume_runner() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cache-volume");
        std::fs::create_dir(&mount).unwrap();

        assert_eq!(
            detect_cache_volume_mount(Some(&mount), [("GITHUB_ACTIONS", "true")]),
            None
        );
    }

    #[test]
    fn returns_mount_for_other_recognised_ci_providers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cache-volume");
        std::fs::create_dir(&mount).unwrap();

        for marker in ["GITLAB_CI", "SEMAPHORE", "JENKINS_URL"] {
            assert_eq!(
                detect_cache_volume_mount(
                    Some(&mount),
                    [(marker, "true"), ("NSC_WORKSPACE", "workspace")],
                ),
                Some(mount.clone()),
                "expected {marker} to be recognised as CI"
            );
        }
    }

    #[test]
    fn ignores_runner_when_mount_is_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("missing-volume");

        assert_eq!(
            detect_cache_volume_mount(
                Some(&mount),
                [("GITHUB_ACTIONS", "true"), ("NSC_WORKSPACE", "workspace")],
            ),
            None
        );
    }

    #[test]
    fn empty_marker_values_do_not_count_as_ci() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cache-volume");
        std::fs::create_dir(&mount).unwrap();

        assert_eq!(
            detect_cache_volume_mount(
                Some(&mount),
                [("GITHUB_ACTIONS", ""), ("NSC_WORKSPACE", "workspace")],
            ),
            None
        );
    }
}
