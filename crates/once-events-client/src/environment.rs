//! Facts about the machine a run happens on that the protocol reports.

/// Variables that only exist inside a job on their provider, so their
/// presence alone settles the question.
///
/// Deliberately excluded: names a developer plausibly exports on their own
/// machine. `BUILD_NUMBER` is used for local artifact versioning, and
/// `JENKINS_URL` is how the Jenkins command line client is pointed at a
/// controller from a laptop. Those are handled by [`JENKINS_JOB_VARIABLES`]
/// below, which requires the pair.
const CI_PROVIDER_VARIABLES: &[&str] = &[
    // GitHub Actions
    "GITHUB_RUN_ID",
    // CircleCI, Bitrise: the two providers the OpenID Connect exchange in
    // `once-cas` already recognises alongside GitHub Actions.
    "CIRCLECI",
    "BITRISE_IO",
    // Buildkite, GitLab, Azure Pipelines, TeamCity
    "BUILDKITE",
    "GITLAB_CI",
    "TF_BUILD",
    "TEAMCITY_VERSION",
    // AWS CodeBuild. Its build number is `CODEBUILD_BUILD_NUMBER`, which is
    // why the bare `BUILD_NUMBER` above would not have matched it.
    "CODEBUILD_BUILD_ID",
    // Travis, AppVeyor, Drone, Bitbucket Pipelines
    "TRAVIS",
    "APPVEYOR",
    "DRONE",
    "BITBUCKET_BUILD_NUMBER",
];

/// Jenkins exports these together from inside a build. Either one alone is
/// something a developer may have set: `JENKINS_URL` to drive the command
/// line client, `BUILD_NUMBER` to stamp a local artifact. Both at once is
/// the job.
const JENKINS_JOB_VARIABLES: &[&str] = &["JENKINS_URL", "BUILD_NUMBER"];

/// Values of `CI` that mean yes, compared without regard to case.
const CI_TRUTHY_VALUES: &[&str] = &["1", "true", "yes", "on"];

/// Whether this run is happening on continuous integration.
///
/// Note that `once-cas` keeps its own, deliberately looser, check for
/// whether it may open a browser during authentication. That one treats a
/// bare `CI` as conclusive because being wrong there only costs a prompt.
/// This one labels data on the dashboard, so it reads `CI` for its value:
/// developers set it locally, and some environments export `CI=false` or
/// empty to say the opposite of what presence implies.
pub fn is_ci() -> bool {
    if CI_PROVIDER_VARIABLES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return true;
    }

    if JENKINS_JOB_VARIABLES
        .iter()
        .all(|name| std::env::var_os(name).is_some())
    {
        return true;
    }

    std::env::var("CI").is_ok_and(|value| {
        let value = value.trim();
        CI_TRUTHY_VALUES
            .iter()
            .any(|truthy| value.eq_ignore_ascii_case(truthy))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `std::env` is process global, so the cases that mutate it share one
    /// lock rather than racing each other across test threads.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores the variables the helper cleared, on the way out of the
    /// scope, so a panicking body cannot leak state into the next test.
    struct RestoreEnv {
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (name, value) in self.previous.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn with_clean_env<T>(vars: &[(&str, &str)], body: impl FnOnce() -> T) -> T {
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let tracked: Vec<&'static str> = CI_PROVIDER_VARIABLES
            .iter()
            .chain(JENKINS_JOB_VARIABLES.iter())
            .copied()
            .chain(std::iter::once("CI"))
            .collect();

        let _restore = RestoreEnv {
            previous: tracked
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect(),
            _guard: guard,
        };

        for name in &tracked {
            std::env::remove_var(name);
        }
        for (name, value) in vars {
            std::env::set_var(name, value);
        }

        body()
    }

    #[test]
    fn a_bare_developer_machine_is_not_ci() {
        assert!(!with_clean_env(&[], is_ci));
    }

    #[test]
    fn a_provider_variable_is_conclusive() {
        for name in CI_PROVIDER_VARIABLES {
            assert!(
                with_clean_env(&[(name, "1")], is_ci),
                "{name} should be conclusive"
            );
        }
    }

    #[test]
    fn a_provider_variable_counts_even_when_empty() {
        // Presence is the signal: these names do not exist off-provider.
        assert!(with_clean_env(&[("GITLAB_CI", "")], is_ci));
    }

    #[test]
    fn code_build_is_recognised_by_its_own_build_id() {
        // CodeBuild names its counter `CODEBUILD_BUILD_NUMBER`, so a bare
        // `BUILD_NUMBER` never matched it.
        assert!(with_clean_env(
            &[
                ("CODEBUILD_BUILD_ID", "job:1"),
                ("CODEBUILD_BUILD_NUMBER", "7")
            ],
            is_ci
        ));
    }

    #[test]
    fn a_jenkins_job_needs_both_of_its_variables() {
        assert!(with_clean_env(
            &[
                ("JENKINS_URL", "https://ci.example.com"),
                ("BUILD_NUMBER", "42")
            ],
            is_ci
        ));

        // Driving the Jenkins command line client from a laptop.
        assert!(!with_clean_env(
            &[("JENKINS_URL", "https://ci.example.com")],
            is_ci
        ));

        // Stamping a local artifact.
        assert!(!with_clean_env(&[("BUILD_NUMBER", "123")], is_ci));
    }

    #[test]
    fn ci_is_read_for_its_value_not_its_presence() {
        for value in [
            "1", "true", "TRUE", "True", "tRuE", "yes", "YES", "on", "ON", " true ",
        ] {
            assert!(with_clean_env(&[("CI", value)], is_ci), "CI={value:?}");
        }

        for value in ["false", "FALSE", "0", "no", "off", ""] {
            assert!(!with_clean_env(&[("CI", value)], is_ci), "CI={value:?}");
        }
    }

    /// Restoration is what keeps these cases from leaking into each other,
    /// including when a body panics, since unwinding still runs `Drop`.
    ///
    /// The guard is exercised directly rather than through
    /// `catch_unwind(with_clean_env(..))`: that would have to touch the
    /// environment outside the lock to plant a value to check, which races
    /// another case's restore, and it would assume `GITHUB_RUN_ID` is unset
    /// in the test process, which is false on GitHub Actions.
    #[test]
    fn the_restore_guard_puts_the_environment_back() {
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = std::env::var_os("GITHUB_RUN_ID");

        {
            let _restore = RestoreEnv {
                previous: vec![("GITHUB_RUN_ID", before.clone())],
                _guard: guard,
            };

            std::env::set_var("GITHUB_RUN_ID", "temporary");
            assert_eq!(
                std::env::var("GITHUB_RUN_ID").ok().as_deref(),
                Some("temporary")
            );
        }

        assert_eq!(std::env::var_os("GITHUB_RUN_ID"), before);
    }
}
