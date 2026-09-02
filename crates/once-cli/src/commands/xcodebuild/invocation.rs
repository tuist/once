use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use once_frontend::GraphTarget;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub(super) quiet: bool,
    scheme: String,
    project: Option<String>,
}

impl Invocation {
    pub(super) fn parse(argv: &[String]) -> Option<Self> {
        let mut parser = Parser::default();
        let mut index = 0;
        while let Some(argument) = argv.get(index) {
            match argument.as_str() {
                "-project" => {
                    parser.project = Some(parser.value(argv, &mut index, ProjectKind::Project)?);
                }
                "-scheme" => {
                    parser.scheme = Some(parser.value(argv, &mut index, ProjectKind::Scheme)?);
                }
                "-configuration" if !parser.debug_configuration => {
                    let configuration =
                        parser.value(argv, &mut index, ProjectKind::Configuration)?;
                    if configuration != "Debug" {
                        return None;
                    }
                    parser.debug_configuration = true;
                }
                "-quiet" if !parser.quiet => parser.quiet = true,
                "build" if !parser.build => parser.build = true,
                _ => return None,
            }
            index += 1;
        }
        parser.finish()
    }

    pub(super) fn target(&self, graph: &[GraphTarget]) -> Option<String> {
        let workspaces = graph
            .iter()
            .filter(|target| target.kind == "xcode_workspace")
            .collect::<Vec<_>>();
        let [workspace] = workspaces.as_slice() else {
            return None;
        };
        if !workspace_uses_debug_configuration(workspace)
            || !project_matches_workspace(self.project.as_ref(), workspace)
        {
            return None;
        }
        let reachable = reachable_target_ids(workspace, graph);
        let matches = graph
            .iter()
            .filter(|target| {
                reachable.contains(&target.label.id)
                    && target.label.name == self.scheme
                    && target
                        .capabilities
                        .iter()
                        .any(|capability| capability.name == "build")
            })
            .map(|target| target.label.id.clone())
            .collect::<Vec<_>>();
        let [target] = matches.as_slice() else {
            return None;
        };
        Some(target.clone())
    }
}

#[derive(Default)]
struct Parser {
    quiet: bool,
    build: bool,
    debug_configuration: bool,
    project: Option<String>,
    project_kind: Option<ProjectKind>,
    scheme: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Project,
    Scheme,
    Configuration,
}

impl Parser {
    fn value(&mut self, argv: &[String], index: &mut usize, kind: ProjectKind) -> Option<String> {
        let value = argv.get(*index + 1)?.clone();
        if value.starts_with('-') {
            return None;
        }
        match kind {
            ProjectKind::Project => {
                if self.project_kind.replace(kind).is_some() {
                    return None;
                }
            }
            ProjectKind::Scheme => {
                if self.scheme.replace(value.clone()).is_some() {
                    return None;
                }
            }
            ProjectKind::Configuration => {}
        }
        *index += 1;
        Some(value)
    }

    fn finish(self) -> Option<Invocation> {
        if !self.debug_configuration {
            return None;
        }
        Some(Invocation {
            quiet: self.quiet,
            scheme: self.scheme?,
            project: self.project,
        })
    }
}

fn workspace_uses_debug_configuration(workspace: &GraphTarget) -> bool {
    workspace
        .attrs
        .get("configuration")
        .and_then(once_frontend::AttrValue::as_str)
        .is_none_or(|configuration| configuration == "Debug")
}

fn project_matches_workspace(project: Option<&String>, workspace: &GraphTarget) -> bool {
    let Some(project) = project else {
        return true;
    };
    let Some(project) = normalized_project_path(project) else {
        return false;
    };
    configured_project(workspace)
        .or_else(|| project_from_sources(&workspace.srcs))
        .is_some_and(|candidate| candidate == project)
}

fn configured_project(workspace: &GraphTarget) -> Option<PathBuf> {
    workspace
        .attrs
        .get("project")
        .and_then(once_frontend::AttrValue::as_str)
        .and_then(normalized_project_path)
}

fn project_from_sources(sources: &[String]) -> Option<PathBuf> {
    let projects = sources
        .iter()
        .filter_map(|source| {
            let path = Path::new(source);
            (path
                .file_name()
                .is_some_and(|name| name == "project.pbxproj"))
            .then(|| path.parent())
            .flatten()
            .and_then(|path| path.to_str())
            .and_then(normalized_project_path)
        })
        .collect::<BTreeSet<_>>();
    let projects = projects.into_iter().collect::<Vec<_>>();
    let [project] = projects.as_slice() else {
        return None;
    };
    Some(project.clone())
}

fn normalized_project_path(project: &str) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in Path::new(project).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(path) => normalized.push(path),
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    matches!(
        normalized
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("xcodeproj")
    )
    .then_some(normalized)
}

fn reachable_target_ids(workspace: &GraphTarget, graph: &[GraphTarget]) -> BTreeSet<String> {
    let dependencies = graph
        .iter()
        .map(|target| {
            (
                target.label.id.as_str(),
                target.dependency_ids().cloned().collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut pending = workspace.dependency_ids().cloned().collect::<Vec<_>>();
    let mut reachable = BTreeSet::new();
    while let Some(target) = pending.pop() {
        if !reachable.insert(target.clone()) {
            continue;
        }
        if let Some(dependencies) = dependencies.get(target.as_str()) {
            pending.extend(dependencies.iter().cloned());
        }
    }
    reachable
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_frontend::{Capability, TargetLabel};

    fn target(id: &str, kind: &str, deps: &[&str], capabilities: &[&str]) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: String::new(),
                name: id.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: deps.iter().map(ToString::to_string).collect(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
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
    fn parses_a_debug_scheme_build() {
        let invocation = Invocation::parse(&arguments(&[
            "-project",
            "Client.xcodeproj",
            "-scheme",
            "Client",
            "-configuration",
            "Debug",
            "-quiet",
            "build",
        ]))
        .unwrap();

        assert!(invocation.quiet);
        assert_eq!(invocation.scheme, "Client");
        assert_eq!(invocation.project.as_deref(), Some("Client.xcodeproj"));
    }

    #[test]
    fn rejects_shapes_that_cannot_preserve_xcode_semantics() {
        for invalid in [
            ["-scheme", "Client", "test"].as_slice(),
            ["-scheme", "Client"].as_slice(),
            ["-scheme", "Client", "-configuration", "Release"].as_slice(),
            ["-workspace", "Client.xcworkspace", "-scheme", "Client"].as_slice(),
            [
                "-scheme",
                "Client",
                "-destination",
                "platform=iOS Simulator",
            ]
            .as_slice(),
            ["-scheme", "Client", "PRODUCT_NAME=Changed"].as_slice(),
        ] {
            assert!(Invocation::parse(&arguments(invalid)).is_none());
        }
    }

    #[test]
    fn selects_only_a_reachable_build_target_from_one_xcode_workspace() {
        let graph = vec![
            target("xcode", "xcode_workspace", &["Client"], &["build"]),
            target("Client", "apple_application", &["Library"], &["build"]),
            target("Library", "apple_library", &[], &["build"]),
            target("Other", "script", &[], &["build"]),
        ];
        let invocation = Invocation::parse(&arguments(&[
            "-scheme",
            "Client",
            "-configuration",
            "Debug",
        ]))
        .unwrap();

        assert_eq!(invocation.target(&graph).as_deref(), Some("Client"));
    }

    #[test]
    fn selects_a_project_only_when_it_matches_the_workspace_seed() {
        let mut workspace = target("xcode", "xcode_workspace", &["Client"], &["build"]);
        workspace.srcs = vec!["Client.xcodeproj/project.pbxproj".to_string()];
        let graph = vec![
            workspace,
            target("Client", "apple_application", &[], &["build"]),
        ];

        let matching = Invocation::parse(&arguments(&[
            "-project",
            "./Client.xcodeproj",
            "-scheme",
            "Client",
            "-configuration",
            "Debug",
        ]))
        .unwrap();
        let other = Invocation::parse(&arguments(&[
            "-project",
            "Other.xcodeproj",
            "-scheme",
            "Client",
            "-configuration",
            "Debug",
        ]))
        .unwrap();

        assert_eq!(matching.target(&graph).as_deref(), Some("Client"));
        assert_eq!(other.target(&graph), None);
    }

    #[test]
    fn requires_an_unambiguous_project_source() {
        assert_eq!(
            project_from_sources(&arguments(&[
                "Client.xcodeproj/project.pbxproj",
                "Other.xcodeproj/project.pbxproj",
            ])),
            None
        );
    }

    #[test]
    fn requires_one_reachable_target_with_the_scheme_name() {
        let mut second_client = target("apps/Client", "apple_application", &[], &["build"]);
        second_client.label.name = "Client".to_string();
        let graph = vec![
            target(
                "xcode",
                "xcode_workspace",
                &["Client", "apps/Client"],
                &["build"],
            ),
            target("Client", "apple_application", &[], &["build"]),
            second_client,
        ];
        let invocation = Invocation::parse(&arguments(&[
            "-scheme",
            "Client",
            "-configuration",
            "Debug",
        ]))
        .unwrap();

        assert_eq!(invocation.target(&graph), None);
    }

    #[test]
    fn rejects_an_ambiguous_or_unreachable_scheme() {
        let graph = vec![
            target("xcode", "xcode_workspace", &["Client"], &["build"]),
            target("other_xcode", "xcode_workspace", &["Other"], &["build"]),
            target("Client", "apple_application", &[], &["build"]),
            target("Other", "apple_application", &[], &["build"]),
        ];
        let invocation = Invocation::parse(&arguments(&[
            "-scheme",
            "Client",
            "-configuration",
            "Debug",
        ]))
        .unwrap();

        assert_eq!(invocation.target(&graph), None);
    }
}
