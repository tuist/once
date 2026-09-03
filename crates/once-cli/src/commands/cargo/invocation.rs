use once_frontend::GraphTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Build,
    Test,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub(super) command: Command,
    pub(super) quiet: bool,
}

pub(super) struct NativePackage {
    pub(super) build_target: String,
    pub(super) test_targets: Vec<String>,
}

impl Invocation {
    pub(super) fn parse(argv: &[String]) -> Option<Self> {
        let mut parser = Parser::default();
        let mut index = 0;
        while let Some(argument) = argv.get(index) {
            match argument.as_str() {
                "build" => parser.command(Command::Build)?,
                "test" => parser.command(Command::Test)?,
                "-q" | "--quiet" if !parser.quiet => parser.quiet = true,
                "--manifest-path" => {
                    let manifest_path = Parser::value(argv, &mut index)?;
                    if !matches!(manifest_path.as_str(), "Cargo.toml" | "./Cargo.toml")
                        || !parser.manifest_path()
                    {
                        return None;
                    }
                }
                "--offline" | "--frozen" | "--locked" => {}
                _ => return None,
            }
            index += 1;
        }
        parser.finish()
    }

    pub(super) fn package(graph: &[GraphTarget]) -> Option<NativePackage> {
        let workspaces = graph
            .iter()
            .filter(|target| target.kind == "cargo_workspace" && has_capability(target, "build"))
            .collect::<Vec<_>>();
        let [workspace] = workspaces.as_slice() else {
            return None;
        };
        let test_targets = graph
            .iter()
            .filter(|target| {
                target.kind == "rust_test"
                    && has_capability(target, "test")
                    && is_first_party_target(target, workspace)
            })
            .map(|target| target.label.id.clone())
            .collect();
        Some(NativePackage {
            build_target: workspace.label.id.clone(),
            test_targets,
        })
    }
}

#[derive(Default)]
struct Parser {
    command: Option<Command>,
    manifest_path: bool,
    quiet: bool,
}

impl Parser {
    fn command(&mut self, command: Command) -> Option<()> {
        self.command.replace(command).is_none().then_some(())
    }

    fn value(argv: &[String], index: &mut usize) -> Option<String> {
        let value = argv.get(*index + 1)?.clone();
        (!value.starts_with('-'))
            .then_some(value)
            .inspect(|_| *index += 1)
    }

    fn manifest_path(&mut self) -> bool {
        !std::mem::replace(&mut self.manifest_path, true)
    }

    fn finish(self) -> Option<Invocation> {
        Some(Invocation {
            command: self.command?,
            quiet: self.quiet,
        })
    }
}

fn has_capability(target: &GraphTarget, capability: &str) -> bool {
    target
        .capabilities
        .iter()
        .any(|candidate| candidate.name == capability)
}

fn is_first_party_target(target: &GraphTarget, workspace: &GraphTarget) -> bool {
    let package_prefix = if workspace.label.package.is_empty() {
        String::new()
    } else {
        format!("{}/", workspace.label.package)
    };
    target.srcs.iter().any(|source| {
        source
            .strip_prefix(&package_prefix)
            .is_some_and(|source| !source.starts_with(".once/"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use once_frontend::{Capability, TargetLabel};

    fn target(
        id: &str,
        package: &str,
        kind: &str,
        srcs: &[&str],
        capabilities: &[&str],
    ) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: package.to_string(),
                name: id.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: srcs.iter().map(ToString::to_string).collect(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: capabilities
                .iter()
                .map(|name| Capability {
                    name: (*name).to_string(),
                    output_groups: Vec::new(),
                    requires_outputs: Vec::new(),
                })
                .collect(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_build_and_test_commands() {
        let build = Invocation::parse(&arguments(&["build"]));
        let test = Invocation::parse(&arguments(&["-q", "test"]));

        assert_eq!(
            build,
            Some(Invocation {
                command: Command::Build,
                quiet: false,
            })
        );
        assert_eq!(
            test,
            Some(Invocation {
                command: Command::Test,
                quiet: true,
            })
        );
    }

    #[test]
    fn accepts_lockfile_flags_and_workspace_manifest_path() {
        let build = Invocation::parse(&arguments(&[
            "build",
            "--locked",
            "--manifest-path",
            "Cargo.toml",
        ]));
        assert_eq!(
            build,
            Some(Invocation {
                command: Command::Build,
                quiet: false,
            })
        );
    }

    #[test]
    fn rejects_requests_that_once_cannot_model() {
        for invalid in [
            ["build", "--release"].as_slice(),
            ["test", "--", "some_test"].as_slice(),
            ["build", "--features", "extra"].as_slice(),
            ["build", "-p", "member"].as_slice(),
            ["build", "--package", "member"].as_slice(),
            ["build", "--target", "aarch64-unknown-linux-gnu"].as_slice(),
            ["check"].as_slice(),
            ["build", "test"].as_slice(),
            ["build", "--manifest-path", "crates/other/Cargo.toml"].as_slice(),
        ] {
            assert!(Invocation::parse(&arguments(invalid)).is_none());
        }
    }

    #[test]
    fn selects_only_first_party_tests_from_one_native_workspace() {
        let graph = vec![
            target("cargo", "", "cargo_workspace", &[], &["build"]),
            target(
                "cargo_hello_bin_hello_unit_tests",
                "",
                "rust_test",
                &["src/main.rs"],
                &["build", "test"],
            ),
            target(
                "third_party_serde_unit_tests",
                "",
                "rust_test",
                &[".once/cargo-packages/serde/src/lib.rs"],
                &["build", "test"],
            ),
        ];

        assert!(Invocation::parse(&arguments(&["test"])).is_some());
        let package = Invocation::package(&graph).unwrap();

        assert_eq!(package.build_target, "cargo");
        assert_eq!(
            package.test_targets,
            ["cargo_hello_bin_hello_unit_tests".to_string()]
        );
    }

    #[test]
    fn rejects_ambiguous_native_workspaces() {
        let graph = vec![
            target("first", "", "cargo_workspace", &[], &["build"]),
            target("second", "", "cargo_workspace", &[], &["build"]),
        ];
        assert!(Invocation::parse(&arguments(&["build"])).is_some());
        assert!(Invocation::package(&graph).is_none());
    }
}
