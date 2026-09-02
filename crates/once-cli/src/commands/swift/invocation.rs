use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{bail, Context, Result};
use once_frontend::GraphTarget;
use serde_json::Value;

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
                "-c" | "--configuration" => {
                    let configuration = Parser::value(argv, &mut index)?;
                    if configuration != "debug" || !parser.debug_configuration() {
                        return None;
                    }
                }
                "--package-path" => {
                    let package_path = Parser::value(argv, &mut index)?;
                    if !matches!(package_path.as_str(), "." | "./") || !parser.package_path() {
                        return None;
                    }
                }
                "-q" | "--quiet" if !parser.quiet => parser.quiet = true,
                _ => return None,
            }
            index += 1;
        }
        parser.finish()
    }

    pub(super) fn package(graph: &[GraphTarget]) -> Option<NativePackage> {
        let workspaces = graph
            .iter()
            .filter(|target| {
                target.kind == "swift_package_workspace" && has_capability(target, "build")
            })
            .collect::<Vec<_>>();
        let [workspace] = workspaces.as_slice() else {
            return None;
        };
        let test_targets = graph
            .iter()
            .filter(|target| {
                target.kind == "apple_test_bundle"
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

pub(super) fn has_unlocked_remote_dependencies(workspace: &Path, swift: &Path) -> Result<bool> {
    if workspace.join("Package.resolved").is_file() {
        return Ok(false);
    }
    let output = ProcessCommand::new(swift)
        .args(["package", "dump-package", "--package-path"])
        .arg(workspace)
        .output()
        .with_context(|| format!("reading Swift package metadata with {}", swift.display()))?;
    if !output.status.success() {
        bail!(
            "Swift Package Manager could not read {}",
            workspace.join("Package.swift").display()
        );
    }
    let package: Value = serde_json::from_slice(&output.stdout)
        .context("decoding Swift Package Manager package metadata")?;
    Ok(package
        .get("dependencies")
        .and_then(Value::as_array)
        .is_some_and(|dependencies| dependencies.iter().any(is_remote_dependency)))
}

fn is_remote_dependency(dependency: &Value) -> bool {
    dependency.get("sourceControl").is_some() || dependency.get("registry").is_some()
}

#[derive(Default)]
struct Parser {
    command: Option<Command>,
    debug_configuration: bool,
    package_path: bool,
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

    fn debug_configuration(&mut self) -> bool {
        !std::mem::replace(&mut self.debug_configuration, true)
    }

    fn package_path(&mut self) -> bool {
        !std::mem::replace(&mut self.package_path, true)
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
    use serde_json::json;

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
    fn parses_debug_build_and_test_commands() {
        let build = Invocation::parse(&arguments(&["build", "--configuration", "debug"]));
        let test = Invocation::parse(&arguments(&["-q", "test", "-c", "debug"]));

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
    fn rejects_requests_that_once_cannot_model() {
        for invalid in [
            ["build", "-c", "release"].as_slice(),
            ["test", "--filter", "NIOTests"].as_slice(),
            ["package", "resolve"].as_slice(),
            ["build", "test"].as_slice(),
            ["build", "--package-path", "Packages/Example"].as_slice(),
        ] {
            assert!(Invocation::parse(&arguments(invalid)).is_none());
        }
    }

    #[test]
    fn selects_only_first_party_tests_from_one_native_package() {
        let graph = vec![
            target(
                "swift_package",
                "",
                "swift_package_workspace",
                &[],
                &["build"],
            ),
            target(
                "SwiftPackage_NIO_NIOTests",
                "",
                "apple_test_bundle",
                &["Tests/NIOTests/Test.swift"],
                &["build", "test"],
            ),
            target(
                "SwiftPackage_Atomics_AtomicsTests",
                "",
                "apple_test_bundle",
                &[".once/swift-package-packages/swift-atomics/Tests/AtomicsTests/Test.swift"],
                &["build", "test"],
            ),
        ];

        assert!(Invocation::parse(&arguments(&["test"])).is_some());
        let package = Invocation::package(&graph);

        assert_eq!(package.unwrap().test_targets, ["SwiftPackage_NIO_NIOTests"]);
    }

    #[test]
    fn rejects_ambiguous_native_packages() {
        let graph = vec![
            target("first", "", "swift_package_workspace", &[], &["build"]),
            target("second", "", "swift_package_workspace", &[], &["build"]),
        ];

        assert!(Invocation::parse(&arguments(&["build"])).is_some());
        assert!(Invocation::package(&graph).is_none());
    }

    #[test]
    fn recognizes_remote_package_metadata() {
        assert!(is_remote_dependency(&json!({
            "sourceControl": [{"identity": "swift-collections"}]
        })));
        assert!(is_remote_dependency(&json!({
            "registry": [{"identity": "swift-collections"}]
        })));
        assert!(!is_remote_dependency(&json!({
            "fileSystem": [{"identity": "local-support"}]
        })));
    }
}
