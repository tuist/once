use once_frontend::{AttrValue, GraphTarget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Build,
    Test,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Invocation {
    pub(super) command: Command,
    label: String,
}

impl Invocation {
    /// Parse a `bazel <command> <label>` invocation. Anything that carries
    /// extra flags, more than one label, or a shape Once cannot yet model
    /// (a wildcard label, `--config`, `--test_filter`, …) returns None so
    /// the caller falls back to running the system `bazel` unchanged.
    pub(super) fn parse(argv: &[String]) -> Option<Self> {
        let mut iter = argv.iter();
        let command = match iter.next()?.as_str() {
            "build" => Command::Build,
            "test" => Command::Test,
            _ => return None,
        };
        let mut label: Option<String> = None;
        for argument in iter {
            if argument == "--" {
                return None;
            }
            if argument.starts_with('-') {
                return None;
            }
            if label.replace(argument.clone()).is_some() {
                return None;
            }
        }
        let label = label?;
        if !is_single_label(&label) {
            return None;
        }
        Some(Self { command, label })
    }

    /// Resolve the Once target id that stands for the requested Bazel label.
    /// The bazel_workspace resolver emits every rule as a bazel_target and
    /// records the original label on `bazel_label`; this is the reverse
    /// lookup that keeps the wrapper honest about what it will build.
    pub(super) fn target(&self, graph: &[GraphTarget]) -> Option<String> {
        let matches = graph
            .iter()
            .filter(|target| {
                is_bazel_target_kind(&target.kind)
                    && target
                        .attrs
                        .get("bazel_label")
                        .and_then(AttrValue::as_str)
                        .is_some_and(|value| value == self.label)
                    && required_capability(target, self.command)
            })
            .map(|target| target.label.id.clone())
            .collect::<Vec<_>>();
        let [target] = matches.as_slice() else {
            return None;
        };
        Some(target.clone())
    }
}

fn is_bazel_target_kind(kind: &str) -> bool {
    matches!(kind, "bazel_target" | "bazel_test" | "bazel_binary")
}

fn required_capability(target: &GraphTarget, command: Command) -> bool {
    let capability = match command {
        Command::Build => "build",
        Command::Test => "test",
    };
    target
        .capabilities
        .iter()
        .any(|candidate| candidate.name == capability)
}

fn is_single_label(value: &str) -> bool {
    if !value.starts_with("//") {
        return false;
    }
    if value.contains("...") {
        return false;
    }
    // A bare `//foo` (package with no explicit name) is ambiguous under
    // Bazel's default-target rule; require the explicit `:name` so the
    // wrapper knows exactly what to hand to Once. This is the same
    // conservative stance the swift/xcodebuild wrappers take.
    value.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use once_frontend::{AttrValue, Capability, TargetLabel};

    fn target(id: &str, kind: &str, label: &str, capabilities: &[&str]) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: String::new(),
                name: id.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::from([(
                "bazel_label".to_string(),
                AttrValue::String(label.to_string()),
            )]),
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
    fn parses_a_build_or_test_with_a_single_labeled_target() {
        let build = Invocation::parse(&arguments(&["build", "//src:kura"])).unwrap();
        let test = Invocation::parse(&arguments(&["test", "//src:kura_lib_test"])).unwrap();

        assert_eq!(build.command, Command::Build);
        assert_eq!(build.label, "//src:kura");
        assert_eq!(test.command, Command::Test);
        assert_eq!(test.label, "//src:kura_lib_test");
    }

    #[test]
    fn rejects_shapes_that_once_cannot_yet_model() {
        for invalid in [
            ["query", "//..."].as_slice(),
            ["build"].as_slice(),
            ["build", "//..."].as_slice(),
            ["build", "//src:a", "//src:b"].as_slice(),
            ["build", "//src:a", "--config=debug"].as_slice(),
            ["build", "--config", "debug", "//src:a"].as_slice(),
            ["run", "//src:kura"].as_slice(),
            ["build", "src:kura"].as_slice(),
            ["build", "//src"].as_slice(),
        ] {
            assert!(Invocation::parse(&arguments(invalid)).is_none());
        }
    }

    #[test]
    fn resolves_the_bazel_target_that_carries_the_matching_label() {
        let graph = vec![
            target("bz_kura", "bazel_binary", "//:kura", &["build", "run"]),
            target(
                "bz_kura_lib_test",
                "bazel_test",
                "//:kura_lib_test",
                &["build", "test"],
            ),
            target("bz_lib", "bazel_target", "//:lib", &["build"]),
        ];
        let build_binary = Invocation::parse(&arguments(&["build", "//:kura"])).unwrap();
        let build_library = Invocation::parse(&arguments(&["build", "//:lib"])).unwrap();
        let test_test = Invocation::parse(&arguments(&["test", "//:kura_lib_test"])).unwrap();

        assert_eq!(build_binary.target(&graph).as_deref(), Some("bz_kura"));
        assert_eq!(build_library.target(&graph).as_deref(), Some("bz_lib"));
        assert_eq!(test_test.target(&graph).as_deref(), Some("bz_kura_lib_test"));
    }

    #[test]
    fn returns_no_target_when_the_capability_is_missing() {
        // A non-test rule advertises only `build`, so `bazel test` on it
        // falls through to the system `bazel` unchanged.
        let graph = vec![target("bz_lib", "bazel_target", "//:lib", &["build"])];
        let test = Invocation::parse(&arguments(&["test", "//:lib"])).unwrap();

        assert_eq!(test.target(&graph), None);
    }
}
