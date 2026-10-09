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

/// Branch variables in the order each provider should be read. A pull or
/// merge request names its source branch separately from the ref the job
/// checked out (GitHub's `GITHUB_REF_NAME` is `123/merge` there), so those
/// come first. `GITHUB_REF_NAME` is also a tag name on tag pushes, which is
/// why it is only read when `GITHUB_REF_TYPE` says it is a branch.
const CI_BRANCH_VARIABLES: &[&str] = &[
    // GitHub Actions
    "GITHUB_HEAD_REF",
    "GITHUB_REF_NAME",
    // GitLab
    "CI_MERGE_REQUEST_SOURCE_BRANCH_NAME",
    "CI_COMMIT_BRANCH",
    // Buildkite, CircleCI, Bitrise, Bitbucket Pipelines
    "BUILDKITE_BRANCH",
    "CIRCLE_BRANCH",
    "BITRISE_GIT_BRANCH",
    "BITBUCKET_BRANCH",
    // Azure Pipelines
    "SYSTEM_PULLREQUEST_SOURCEBRANCH",
    "BUILD_SOURCEBRANCH",
    // Travis, AppVeyor, Drone
    "TRAVIS_PULL_REQUEST_BRANCH",
    "TRAVIS_BRANCH",
    "APPVEYOR_PULL_REQUEST_HEAD_REPO_BRANCH",
    "APPVEYOR_REPO_BRANCH",
    "DRONE_SOURCE_BRANCH",
    "DRONE_BRANCH",
    // AWS CodeBuild
    "CODEBUILD_WEBHOOK_HEAD_REF",
    // Jenkins multibranch pipelines
    "CHANGE_BRANCH",
    "BRANCH_NAME",
];

/// The branch a CI job was started for, as its provider reports it.
///
/// CI checkouts are usually a detached HEAD, where git has no branch name
/// to give, so the provider's variable is the only reliable source there.
/// Off CI this returns `None` and the caller asks git instead: a developer
/// may have one of these names exported from an unrelated job.
pub fn ci_branch() -> Option<String> {
    if !is_ci() {
        return None;
    }
    ci_branch_from(|name| std::env::var(name).ok())
}

fn ci_branch_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    CI_BRANCH_VARIABLES.iter().find_map(|name| {
        if *name == "GITHUB_REF_NAME" && lookup("GITHUB_REF_TYPE").as_deref() != Some("branch") {
            return None;
        }
        let tag_variable = match *name {
            "BUILDKITE_BRANCH" => Some("BUILDKITE_TAG"),
            "CIRCLE_BRANCH" => Some("CIRCLE_TAG"),
            "BITBUCKET_BRANCH" => Some("BITBUCKET_TAG"),
            "TRAVIS_BRANCH" => Some("TRAVIS_TAG"),
            "DRONE_BRANCH" => Some("DRONE_TAG"),
            "BRANCH_NAME" => Some("TAG_NAME"),
            "BITRISE_GIT_BRANCH" => Some("BITRISE_GIT_TAG"),
            _ => None,
        };
        if tag_variable.is_some_and(|tag| lookup(tag).is_some_and(|value| !value.trim().is_empty()))
            || (*name == "APPVEYOR_REPO_BRANCH"
                && lookup("APPVEYOR_REPO_TAG")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true")))
        {
            return None;
        }
        let value = lookup(name)?;
        let branch = value.trim();
        let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        (!branch.is_empty() && !branch.starts_with("refs/")).then(|| branch.to_string())
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

    fn branch_from(vars: &[(&str, &str)]) -> Option<String> {
        let vars: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        ci_branch_from(|name| vars.get(name).cloned())
    }

    #[test]
    fn a_pull_request_reports_its_source_branch_not_the_merge_ref() {
        assert_eq!(
            branch_from(&[
                ("GITHUB_HEAD_REF", "feat/cache"),
                ("GITHUB_REF_NAME", "42/merge"),
                ("GITHUB_REF_TYPE", "branch"),
            ]),
            Some("feat/cache".to_string())
        );
    }

    #[test]
    fn a_push_reports_the_pushed_branch() {
        assert_eq!(
            branch_from(&[
                ("GITHUB_HEAD_REF", ""),
                ("GITHUB_REF_NAME", "main"),
                ("GITHUB_REF_TYPE", "branch"),
            ]),
            Some("main".to_string())
        );
    }

    #[test]
    fn other_provider_tags_do_not_become_branches() {
        for variable in ["BUILD_SOURCEBRANCH", "CODEBUILD_WEBHOOK_HEAD_REF"] {
            assert_eq!(branch_from(&[(variable, "refs/tags/v1.2.3")]), None);
            assert_eq!(branch_from(&[(variable, "refs/pull/42/merge")]), None);
        }
        for (branch, tag, value) in [
            ("BUILDKITE_BRANCH", "BUILDKITE_TAG", "v1.2.3"),
            ("CIRCLE_BRANCH", "CIRCLE_TAG", "v1.2.3"),
            ("BITBUCKET_BRANCH", "BITBUCKET_TAG", "v1.2.3"),
            ("TRAVIS_BRANCH", "TRAVIS_TAG", "v1.2.3"),
            ("DRONE_BRANCH", "DRONE_TAG", "v1.2.3"),
            ("BRANCH_NAME", "TAG_NAME", "v1.2.3"),
            ("BITRISE_GIT_BRANCH", "BITRISE_GIT_TAG", "v1.2.3"),
            ("APPVEYOR_REPO_BRANCH", "APPVEYOR_REPO_TAG", "true"),
        ] {
            assert_eq!(branch_from(&[(branch, "v1.2.3"), (tag, value)]), None);
        }
    }

    #[test]
    fn a_tag_push_has_no_branch() {
        assert_eq!(
            branch_from(&[("GITHUB_REF_NAME", "1.2.3"), ("GITHUB_REF_TYPE", "tag")]),
            None
        );
    }

    #[test]
    fn full_refs_are_reduced_to_the_branch_name() {
        assert_eq!(
            branch_from(&[("BUILD_SOURCEBRANCH", "refs/heads/release/1.0")]),
            Some("release/1.0".to_string())
        );
        assert_eq!(
            branch_from(&[("CODEBUILD_WEBHOOK_HEAD_REF", "refs/heads/main")]),
            Some("main".to_string())
        );
    }

    #[test]
    fn no_branch_variable_means_no_branch() {
        assert_eq!(branch_from(&[]), None);
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
