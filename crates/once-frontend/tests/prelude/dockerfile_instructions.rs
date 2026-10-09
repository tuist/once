use super::*;

#[test]
fn default_docker_builder_provisions_a_container_builder() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
def _resolve_host_executable(name):
    return "/tools/docker"
def host_which(name):
    return "/tools/python3"
def host_env(name):
    return ""
def host_command(argv, merge_stderr = False, check = True):
    if argv[1:3] == ["buildx", "version"]:
        return "buildx 1.0"
    if argv[1:3] == ["buildx", "inspect"]:
        if len(argv) > 3 and argv[3].startswith("once-images-"):
            return "Driver: docker-container\nBuildKit version: v1.0"
        return "Driver: docker\nBuildKit version: v1.0"
    if argv[1:3] == ["context", "inspect"]:
        return "endpoint"
    if argv[1:3] == ["buildx", "create"]:
        if check:
            fail("create must tolerate an existing builder")
        return ""
    fail("unexpected command: " + repr(argv))
ctx = {{"attr": {{}}, "label": {{"id": "image"}}}}
result = repr(_dockerfile_toolchain(ctx, auto_builder = True))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(workspace.path().to_path_buf(), String::new(), String::new());
    let (_, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let result = result.unwrap();
    assert!(result.contains("docker-container"), "{result}");
    assert!(result.contains("once-images-"), "{result}");
}

fn analyze(recipe: &str, capability: &str) -> (AnalysisStore, String) {
    analyze_with(
        recipe,
        capability,
        r#"{"platform": "linux/arm64", "pull": False, "cacheable": True}"#,
    )
    .unwrap()
}

fn analyze_with(
    recipe: &str,
    capability: &str,
    attrs: &str,
) -> Result<(AnalysisStore, String), String> {
    let workspace = TempDir::new().unwrap();
    std::fs::write(workspace.path().join("Dockerfile"), recipe).unwrap();
    std::fs::write(workspace.path().join("early.txt"), "early").unwrap();
    std::fs::write(workspace.path().join("late.txt"), "late").unwrap();
    let source = format!(
        r#"{}
def _dockerfile_toolchain(ctx, auto_builder = False):
    return ("/tools/docker", "docker-test", "remote", "remote-builder")
def host_which(name):
    return "/tools/" + name
def host_command(argv, merge_stderr = False, check = True):
    return "Python 3.14"
ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {attrs},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/image",
    "capability": {capability:?},
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".into(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.map(|result| (store, result))
}

fn written(store: &AnalysisStore, suffix: &str) -> String {
    store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::WriteFile { path, bytes }) if path.ends_with(suffix) => {
                Some(String::from_utf8(bytes.clone()).unwrap())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing file {suffix}"))
}

#[test]
fn dockerfile_instructions_have_precise_sources_and_stage_edges() {
    let (store, provider) = analyze(
        "FROM scratch AS base\nCOPY early.txt /early\nFROM base AS build\nCOPY late.txt /late\nFROM scratch\nCOPY --from=build /late /late\nCMD [\"/late\"]\n", "build",
    );
    assert!(provider.contains("instruction-plan.json"));
    let early = action_by_identifier(&store, "image:2:copy");
    assert!(early.inputs.iter().any(|path| path == "early.txt"));
    assert!(!early
        .inputs
        .iter()
        .any(|path| path == "late.txt" || path == "Dockerfile"));
    assert!(!early.depends_on_prior_actions);
    assert!(early.cacheable);
    let base = action_by_identifier(&store, "image:3:from");
    assert!(base
        .inputs
        .iter()
        .any(|path| path.ends_with("instructions/2/image")));
    let cross = action_by_identifier(&store, "image:6:copy");
    assert!(cross
        .inputs
        .iter()
        .any(|path| path.ends_with("instructions/4/image")));
    assert!(cross
        .inputs
        .iter()
        .any(|path| path.ends_with("instructions/5/image")));
    let argv = cross.argv.join(" ");
    assert!(!argv.contains("rewrite-timestamp"), "{argv}");
    assert!(
        argv.contains(
            "--build-context once_source=oci-layout://.once/out/image/instructions/4/image"
        ),
        "{argv}"
    );
    assert!(
        argv.contains(
            "--build-context once_previous=oci-layout://.once/out/image/instructions/5/image"
        ),
        "{argv}"
    );
    assert!(!argv.contains("python"), "{argv}");
    let export = action_by_identifier(&store, "image:dockerfile-export");
    assert!(export.argv.join(" ").contains("rewrite-timestamp=true"));
    assert!(written(&store, "instructions/6/Dockerfile")
        .contains("COPY --from=once_source /late /late"));
}

#[test]
fn dockerfile_stage_references_ignore_case() {
    let (store, _) = analyze(
        "FROM scratch AS Builder\nCOPY early.txt /early\nFROM scratch\nCOPY --from=builder /early /early\n",
        "build",
    );
    assert!(store
        .actions
        .iter()
        .any(|action| action.identifier.as_deref() == Some("image:2:copy")));
    assert!(written(&store, "instructions/4/Dockerfile")
        .contains("COPY --from=once_source /early /early"));
}

#[test]
fn dockerfile_mount_rewrites_exact_stage_names() {
    let (store, _) = analyze(
        "FROM scratch AS base\nCOPY early.txt /early\nFROM scratch AS base_tools\nCOPY late.txt /late\nFROM scratch\nRUN --mount=from=base,target=/a --mount=from=base_tools,target=/b echo from=base,\n",
        "build",
    );
    let recipe = written(&store, "instructions/6/Dockerfile");
    assert!(recipe.contains("from=once_stage_0,target=/a"), "{recipe}");
    assert!(recipe.contains("from=once_stage_1,target=/b"), "{recipe}");
    assert!(recipe.contains("echo from=base,"), "{recipe}");
}

#[test]
fn dockerfile_stage_platform_is_kept_on_later_instructions() {
    let (store, _) = analyze(
        "FROM --platform=linux/amd64 scratch AS build\nCOPY early.txt /early\nFROM scratch\nCOPY --from=build /early /early\n",
        "build",
    );
    assert!(written(&store, "instructions/2/Dockerfile")
        .contains("FROM --platform=linux/amd64 once_previous"));
}

#[test]
fn dockerfile_instructions_preserve_argument_scope_and_continuations() {
    let (store, _) = analyze("ARG BASE=scratch\nFROM ${BASE} AS base\nARG MODE=release\nENV MODE=$MODE\nFROM base\nRUN echo $MODE \\\n    && echo done\n", "build");
    let recipe = written(&store, "instructions/6/Dockerfile");
    assert!(recipe.contains("ARG MODE=\"release\""), "{recipe}");
    assert!(recipe.contains("RUN echo $MODE"));
    assert!(recipe.contains("&& echo done"));
    assert!(!recipe.contains("ENV MODE=$MODE"));
}

#[test]
fn dockerfile_lint_reports_advisory_repairs_without_building() {
    let (store, provider) = analyze("FROM debian:stable\nRUN apt-get update && apt-get install -y curl\nRUN curl https://example.com/install | sh\nRUN --mount=type=secret,id=token cat /run/secrets/token\n", "lint");
    assert!(provider.contains("lint_info"));
    assert!(store
        .actions
        .iter()
        .all(|action| action.operation.is_some()));
    let findings: serde_json::Value =
        serde_json::from_str(&written(&store, "native-results.json")).unwrap();
    assert_eq!(findings.as_array().unwrap().len(), 4);
    assert_eq!(findings[0]["code"], "mutable_base_image");
    assert_eq!(findings[0]["line"], 1);
    assert_eq!(findings[0]["severity"], "warning");
    assert!(findings[1]["repairs"][0]
        .as_str()
        .unwrap()
        .contains("false positive"));
}

#[test]
fn dockerfile_parser_reports_unsupported_syntax_with_exact_fallback() {
    for (recipe, reason) in [
        ("FROM scratch\nONBUILD COPY . /\n", "ONBUILD"),
        ("FROM scratch\nRUN <<EOF\necho hello\nEOF\n", "heredoc"),
        (
            "# syntax=docker/dockerfile:1\nFROM scratch\n",
            "syntax frontend",
        ),
        ("FROM scratch\nARG a=1 b=2\n", "multiple ARG"),
        ("ARG v=1\nARG w=${v:-2}\nFROM scratch\n", "extended ARG"),
        ("# escape=`\nFROM scratch\n", "escape character"),
        ("FROM\n", "without an image"),
        ("# check=error=true\nFROM scratch\n", "check directive"),
        ("FROM scratch\nARG ARCH=$TARGETARCH\n", "automatic platform"),
        (
            "FROM scratch AS base\nFROM scratch\nCOPY --from=\"base\" /a /a\n",
            "quoted --from",
        ),
        (
            "ARG S=base\nFROM scratch AS base\nFROM scratch\nCOPY --from=$S /a /a\n",
            "variable stage",
        ),
    ] {
        let source = format!(
            "{}\nresult = _dockerfile_unsupported_message(_dockerfile_first_unsupported(_dockerfile_instructions({recipe:?})))",
            dockerfile_prelude_source()
        );
        let message = eval_prelude_source_to_repr(source).unwrap();
        assert!(message.contains(reason), "{message}");
        assert!(
            message.contains("execution_mode = \"buildkit\""),
            "{message}"
        );
    }
}

#[test]
fn dockerfile_instructions_skip_unreachable_stages() {
    let (store, _) = analyze("FROM unavailable.example/not-an-input AS unused\nRUN exit 1\nFROM scratch AS final\nCOPY early.txt /early\n", "build");
    assert!(!store
        .actions
        .iter()
        .any(|action| action.identifier.as_deref() == Some("image:1:from")));
    assert!(!store
        .actions
        .iter()
        .any(|action| action.identifier.as_deref() == Some("image:2:run")));
    assert!(store
        .actions
        .iter()
        .any(|action| action.identifier.as_deref() == Some("image:3:from")));
}

#[test]
fn dockerfile_parser_handles_tabs_and_global_stage_arguments() {
    let (store, _) = analyze("ARG PARENT=base\nFROM\tscratch AS base\nCOPY early.txt /early\nFROM ${PARENT}\nCOPY late.txt /late\n", "build");
    let from = action_by_identifier(&store, "image:4:from");
    assert!(from
        .inputs
        .iter()
        .any(|path| path.ends_with("instructions/3/image")));
    assert!(written(&store, "instructions/4/Dockerfile").contains("FROM once_previous"));
}

#[test]
fn dockerfile_copy_json_retains_paths_with_spaces() {
    let source = format!(
        r#"{}
ctx = {{"label": {{"package": "app", "name": "image", "id": "app/image"}}}}
instruction = {{"opcode": "COPY", "argument": "[\"some file.txt\", \"/file.txt\"]"}}
result = repr(_dockerfile_copy_inputs(ctx, instruction, ".", ["app/some file.txt", "app/unrelated.txt"]))
"#,
        dockerfile_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[\"app/some file.txt\"]"
    );
}

#[test]
fn discovered_dockerfile_uses_its_full_context() {
    let source = format!(
        r#"{}
ctx = {{"label": {{"package": "", "name": "image", "id": "image"}}, "attr": {{"resolver_inputs": ["Dockerfile"]}}, "srcs": ["Dockerfile"]}}
result = repr(_dockerfile_inputs(ctx, "Dockerfile", "."))
"#,
        dockerfile_prelude_source()
    );
    let workspace = TempDir::new().unwrap();
    std::fs::write(workspace.path().join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(workspace.path().join("message.txt"), "hello").unwrap();
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".into(),
    );
    let result = with_active_store(store, || eval_prelude_source_to_repr(source))
        .1
        .unwrap();
    assert!(result.contains("message.txt"), "{result}");
    assert!(result.contains("context"), "{result}");
}

#[test]
fn dockerfile_cache_mounts_do_not_capture_unrelated_context_sources() {
    let (store, _) = analyze("FROM scratch\nRUN --mount=type=cache,target=/cache true\nRUN --mount=type=bind,target=/src true\n", "build");
    let cache_mount = action_by_identifier(&store, "image:2:run");
    assert!(!cache_mount.inputs.iter().any(|path| path == "early.txt"));
    let bind_mount = action_by_identifier(&store, "image:3:run");
    assert!(bind_mount.inputs.iter().any(|path| path == "early.txt"));
}

#[test]
fn dockerfile_comments_change_source_locations_without_changing_actions() {
    let (original, _) = analyze("FROM scratch\nCOPY early.txt /early\n", "build");
    let (commented, _) = analyze(
        "# explanatory comment\nFROM scratch\n\nCOPY early.txt /early\n",
        "build",
    );
    assert_eq!(
        action_by_identifier(&original, "image:2:copy").argv,
        action_by_identifier(&commented, "image:2:copy").argv
    );
    assert_eq!(
        written(&original, "instructions/2/Dockerfile"),
        written(&commented, "instructions/2/Dockerfile")
    );
    let plan: serde_json::Value =
        serde_json::from_str(&written(&commented, "instruction-plan.json")).unwrap();
    assert_eq!(plan["instructions"][1]["line"], 4);
}

#[test]
fn dockerfile_independent_stages_are_adjacent_for_bounded_scheduling() {
    let (store, _) = analyze("FROM scratch AS a\nCOPY early.txt /early\nFROM scratch AS b\nCOPY late.txt /late\nFROM a\nCOPY --from=b /late /late\n", "build");
    let commands = store
        .actions
        .iter()
        .filter(|action| action.operation.is_none())
        .map(|action| action.identifier.as_deref().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        commands,
        vec![
            "image:1:from",
            "image:3:from",
            "image:2:copy",
            "image:4:copy",
            "image:5:from",
            "image:6:copy",
            "image:dockerfile-export"
        ]
    );
}

#[test]
fn dockerfile_lint_and_metadata_survive_extended_syntax() {
    let recipe = "# syntax=docker/dockerfile:1\nFROM debian:stable\nRUN <<EOF\napt-get update\nEOF\nONBUILD RUN true\n";
    for mode in ["auto", "buildkit", "instructions"] {
        let attrs = format!(r#"{{"execution_mode": "{mode}"}}"#);
        let (store, _) = analyze_with(recipe, "lint", &attrs).unwrap();
        let lint = written(&store, "native-results.json");
        assert!(lint.contains("mutable_syntax_frontend"), "{mode}: {lint}");
        assert!(lint.contains("mutable_base_image"), "{mode}: {lint}");
        assert_eq!(
            lint.contains("whole_file_execution"),
            mode == "auto",
            "{mode}: {lint}"
        );
        analyze_with(recipe, "metadata", &attrs).unwrap();
    }
}

#[test]
fn dockerfile_auto_mode_falls_back_with_a_reason() {
    let (_, metadata) =
        analyze_with("FROM scratch\nRUN <<EOF\ntrue\nEOF\n", "metadata", "{}").unwrap();
    assert!(
        metadata.contains("\"execution_mode\": \"buildkit\""),
        "{metadata}"
    );
    assert!(
        metadata.contains("line 2 uses heredoc syntax"),
        "{metadata}"
    );
    let (_, metadata) =
        analyze_with("FROM scratch\nCOPY late.txt /late\n", "metadata", "{}").unwrap();
    assert!(
        metadata.contains("\"execution_mode\": \"instructions\""),
        "{metadata}"
    );
    let (_, metadata) =
        analyze_with("FROM scratch\n", "metadata", r#"{"export_cache": True}"#).unwrap();
    assert!(
        metadata.contains("export_cache requires whole-file execution"),
        "{metadata}"
    );
}

#[test]
fn dockerfile_explicit_instruction_mode_rejects_extended_syntax_when_building() {
    let error = analyze_with(
        "FROM scratch\nONBUILD RUN true\n",
        "build",
        r#"{"execution_mode": "instructions"}"#,
    )
    .err()
    .unwrap();
    assert!(error.contains("line 2: ONBUILD instructions"), "{error}");
}

#[test]
fn dockerfile_cacheable_requires_digest_pinned_reachable_bases() {
    let error = analyze_with(
        "FROM alpine:3.23\nCOPY early.txt /early\n",
        "build",
        r#"{"platform": "linux/arm64", "pull": False, "cacheable": True}"#,
    )
    .err()
    .unwrap();
    assert!(error.contains("name@sha256:<digest>"), "{error}");
    let error = analyze_with("FROM alpine:3.23\n", "build", r#"{"platform": "linux/arm64", "pull": False, "cacheable": True, "execution_mode": "buildkit"}"#)
        .err()
        .unwrap();
    assert!(error.contains("pinned by digest"), "{error}");
    analyze_with("FROM alpine:3.23@sha256:0000000000000000000000000000000000000000000000000000000000000000\n", "build", r#"{"platform": "linux/arm64", "pull": False, "cacheable": True}"#).unwrap();
}

#[test]
fn dockerfile_continuations_keep_inner_whitespace() {
    let (store, _) = analyze("FROM scratch\nLABEL a=\"x \\\n    y\"\n", "build");
    assert!(written(&store, "instructions/2/Dockerfile").contains("x     y"));
}

const PINNED: &str = r#"{"platform": "linux/arm64", "pull": False, "cacheable": True}"#;

#[test]
fn dockerfile_later_global_defaults_win_for_pinning_and_supplied_arguments_win_over_both() {
    let error = analyze_with(
        "ARG BASE=scratch\nARG BASE=alpine:latest\nFROM $BASE\n",
        "build",
        PINNED,
    )
    .err()
    .unwrap();
    assert!(error.contains("alpine:latest"), "{error}");
    analyze_with(
        "ARG BASE=alpine:latest\nFROM $BASE\n",
        "build",
        r#"{"platform": "linux/arm64", "pull": False, "cacheable": True, "build_args": {"BASE": "scratch"}}"#,
    )
    .unwrap();
}

#[test]
fn dockerfile_entrypoint_keeps_a_command_declared_earlier_in_the_stage() {
    let (store, _) = analyze(
        "FROM scratch\nCMD [\"hello\"]\nRUN true\nENTRYPOINT [\"echo\"]\n",
        "build",
    );
    let recipe = written(&store, "instructions/4/Dockerfile");
    assert!(
        recipe.contains("ENTRYPOINT [\"echo\"]\nCMD [\"hello\"]"),
        "{recipe}"
    );
    let (store, _) = analyze(
        "FROM scratch\nCMD [\"hello\"]\nENTRYPOINT [\"echo\"]\n",
        "build",
    );
    let recipe = written(&store, "instructions/3/Dockerfile");
    assert!(
        recipe.contains("CMD [\"hello\"]\nENTRYPOINT [\"echo\"]"),
        "{recipe}"
    );
    assert_eq!(recipe.matches("CMD").count(), 1, "{recipe}");
    let (store, _) = analyze("FROM scratch\nENTRYPOINT [\"echo\"]\n", "build");
    assert!(!written(&store, "instructions/2/Dockerfile").contains("CMD"));
}

#[test]
fn dockerfile_variable_mount_types_and_unmatched_sources_keep_the_full_context() {
    let (store, _) = analyze(
        "FROM scratch\nARG KIND=bind\nRUN --mount=type=$KIND,target=/src true\n",
        "build",
    );
    let action = action_by_identifier(&store, "image:3:run");
    assert!(action.inputs.iter().any(|path| path == "early.txt"));
    assert!(action.inputs.iter().any(|path| path == "late.txt"));
    let (store, _) = analyze("FROM scratch\nCOPY linked/file.txt /file.txt\n", "build");
    let action = action_by_identifier(&store, "image:2:copy");
    assert!(action.inputs.iter().any(|path| path == "early.txt"));
}

#[test]
fn dockerfile_export_keeps_global_arguments_used_by_the_platform() {
    let (store, _) = analyze(
        "ARG P=linux/arm64\nFROM --platform=$P scratch\nCOPY early.txt /e\n",
        "build",
    );
    let recipe = written(&store, "export/Dockerfile");
    assert!(
        recipe.starts_with("ARG P=linux/arm64\nFROM --platform=$P once_previous"),
        "{recipe}"
    );
}

#[test]
fn dockerfile_lint_reports_copy_sources_missing_from_the_context() {
    let (store, _) = analyze_with(
        "FROM scratch\nCOPY early.txt /e\nCOPY ./backend/app /app\nCOPY --from=x /y /y\n",
        "lint",
        "{}",
    )
    .unwrap();
    let lint = written(&store, "native-results.json");
    assert!(lint.contains("missing_copy_source"), "{lint}");
    assert!(lint.contains("backend/app"), "{lint}");
    assert!(!lint.contains("early.txt is not"), "{lint}");
}

#[test]
fn dockerfile_reproducible_layers_rewrite_each_snapshot_as_it_is_exported() {
    let attrs = r#"{"platform": "linux/arm64", "pull": False, "cacheable": True, "reproducible_layers": True}"#;
    let (store, _) = analyze_with("FROM scratch\nCOPY early.txt /e\n", "build", attrs).unwrap();
    assert!(action_by_identifier(&store, "image:2:copy")
        .argv
        .join(" ")
        .contains("tar=false,dest=.once/out/image/instructions/2/image,rewrite-timestamp=true"));
    let (store, _) = analyze("FROM scratch\nCOPY early.txt /e\n", "build");
    assert!(!action_by_identifier(&store, "image:2:copy")
        .argv
        .join(" ")
        .contains("rewrite-timestamp"));
}

#[test]
fn dockerfile_steps_without_context_files_build_against_an_empty_staged_directory() {
    let (store, _) = analyze("FROM scratch\nCOPY early.txt /e\nRUN true\n", "build");
    let from = action_by_identifier(&store, "image:1:from");
    assert_eq!(
        from.create_dirs,
        vec![".once/tmp/image/empty-context".to_string()]
    );
    assert_eq!(from.argv.last().unwrap(), ".once/tmp/image/empty-context");
    let copy = action_by_identifier(&store, "image:2:copy");
    assert!(copy.create_dirs.is_empty());
    assert_eq!(copy.argv.last().unwrap(), ".");
    let run = action_by_identifier(&store, "image:3:run");
    assert_eq!(run.argv.last().unwrap(), ".once/tmp/image/empty-context");
}

#[test]
fn dockerfile_env_assignments_in_one_instruction_see_the_previous_environment() {
    let (store, _) = analyze(
        "FROM scratch\nENV K=old\nENV K=new V=$K\nARG R=$V\nRUN true\n",
        "build",
    );
    let recipe = written(&store, "instructions/5/Dockerfile");
    assert!(recipe.contains("ARG R=\"old\""), "{recipe}");
}

#[test]
fn dockerfile_bracket_globs_are_not_mistaken_for_json_arrays() {
    let (store, _) = analyze(
        "FROM scratch\nCOPY [ab].txt /ctx/\nCOPY [\"early.txt\", \"/dst/\"]\n",
        "build",
    );
    let glob = action_by_identifier(&store, "image:2:copy");
    assert!(glob.inputs.iter().any(|path| path == "early.txt"));
    assert!(glob.inputs.iter().any(|path| path == "late.txt"));
    let json = action_by_identifier(&store, "image:3:copy");
    assert!(json.inputs.iter().any(|path| path == "early.txt"));
    assert!(!json.inputs.iter().any(|path| path == "late.txt"));
}

#[test]
fn dockerfile_malformed_from_is_reported_not_crashed_on() {
    let (store, _) = analyze_with("FROM\nRUN true\n", "lint", "{}").unwrap();
    assert!(written(&store, "native-results.json").contains("whole_file_execution"));
    let error = analyze_with("FROM\n", "build", r#"{"execution_mode": "instructions"}"#)
        .err()
        .unwrap();
    assert!(error.contains("without an image"), "{error}");
}

#[test]
fn dockerfile_cacheable_requires_pinned_images_copied_from_other_images() {
    let attrs = r#"{"platform": "linux/arm64", "pull": False, "cacheable": True}"#;
    let error = analyze_with("FROM scratch\nCOPY --from=nginx:1 /a /a\n", "build", attrs)
        .err()
        .unwrap();
    assert!(error.contains("images copied from"), "{error}");
    let error = analyze_with(
        "FROM scratch\nRUN --mount=type=bind,from=nginx:1,target=/m true\n",
        "build",
        attrs,
    )
    .err()
    .unwrap();
    assert!(error.contains("images copied from"), "{error}");
    analyze_with("FROM scratch AS a\nFROM scratch\nCOPY --from=a /a /a\nCOPY --from=0 /a /b\nCOPY --from=nginx:1@sha256:0000000000000000000000000000000000000000000000000000000000000000 /c /c\n", "build", attrs).unwrap();
}

#[test]
fn dockerfile_argument_defaults_the_resolver_cannot_decode_select_whole_file_execution() {
    for recipe in [
        "FROM scratch\nARG X=hello\nENV E=\\$X\nARG V=$E\n",
        "FROM scratch\nENV K \"hello   world\"\nARG V=$K\n",
        "FROM scratch\nARG K=hello\" world\"\n",
    ] {
        let (_, metadata) =
            analyze_with(recipe, "metadata", r#"{"execution_mode": "instructions"}"#).unwrap();
        assert!(
            metadata.contains("cannot evaluate"),
            "{recipe:?}: {metadata}"
        );
        let (_, auto) = analyze_with(recipe, "metadata", "{}").unwrap();
        assert!(
            auto.contains("\"execution_mode\": \"buildkit\""),
            "{recipe:?}: {auto}"
        );
    }
}

#[cfg(unix)]
#[test]
fn dockerfile_copying_a_symlink_declares_its_target_and_empty_directories_are_inputs() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(
        workspace.path().join("Dockerfile"),
        "FROM scratch\nCOPY link.txt /value\nCOPY tree/ /tree/\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("a.txt"), "alpha").unwrap();
    std::os::unix::fs::symlink("a.txt", workspace.path().join("link.txt")).unwrap();
    std::fs::create_dir_all(workspace.path().join("tree/empty")).unwrap();
    std::fs::write(workspace.path().join("tree/zero"), "").unwrap();
    std::fs::write(workspace.path().join("late.txt"), "late").unwrap();
    let source = format!(
        r#"{}
def _dockerfile_toolchain(ctx, auto_builder = False):
    return ("/tools/docker", "docker-test", "remote", "remote-builder")
def host_which(name):
    return "/tools/" + name
def host_command(argv, merge_stderr = False, check = True):
    return ""
ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{"execution_mode": "instructions"}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".into(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let link = action_by_identifier(&store, "image:2:copy");
    assert!(
        link.inputs.iter().any(|path| path == "link.txt"),
        "{:?}",
        link.inputs
    );
    assert!(
        link.inputs.iter().any(|path| path == "a.txt"),
        "{:?}",
        link.inputs
    );
    assert!(
        !link.inputs.iter().any(|path| path == "late.txt"),
        "{:?}",
        link.inputs
    );
    let tree = action_by_identifier(&store, "image:3:copy");
    assert!(
        tree.inputs.iter().any(|path| path == "tree/empty"),
        "{:?}",
        tree.inputs
    );
    assert!(
        tree.inputs.iter().any(|path| path == "tree/zero"),
        "{:?}",
        tree.inputs
    );
}

#[test]
fn dockerfile_auto_mode_selects_whole_file_execution_for_bases_with_onbuild_triggers() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(
        workspace.path().join("Dockerfile"),
        "FROM legacy:1\nCOPY a.txt /a\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("a.txt"), "alpha").unwrap();
    for (mode, expectation) in [("auto", "fallback"), ("instructions", "error")] {
        let source = format!(
            r#"{}
def _dockerfile_toolchain(ctx, auto_builder = False):
    return ("/tools/docker", "docker-test", "remote", "remote-builder")
def host_command(argv, merge_stderr = False, check = True):
    if argv[1:3] == ["image", "inspect"]:
        return '["COPY . /app"]'
    return ""
ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{"execution_mode": "{mode}", "pull": False}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
            dockerfile_prelude_source()
        );
        let store = AnalysisStore::new(
            workspace.path().to_path_buf(),
            String::new(),
            ".once/out/image".into(),
        );
        let (_, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
        if expectation == "fallback" {
            let provider = result.unwrap();
            assert!(
                provider.contains("\"execution_mode\": \"buildkit\""),
                "{provider}"
            );
            assert!(provider.contains("ONBUILD triggers"), "{provider}");
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("dockerfile_onbuild_base"), "{error}");
        }
    }
}

#[test]
fn dockerfile_mount_sources_do_not_accept_numeric_stage_indexes_as_docker_does() {
    let recipe = "FROM scratch AS a\nRUN true\nFROM scratch\nRUN --mount=from=0,source=/x,target=/x true\nCOPY --from=0 /x /y\n";
    let (store, _) = analyze_with(recipe, "build", r#"{"pull": False}"#).unwrap();
    let error = analyze_with(
        recipe,
        "build",
        r#"{"platform": "linux/arm64", "pull": False, "cacheable": True}"#,
    )
    .err()
    .unwrap();
    assert!(error.contains("images copied from"), "{error}");
    let mount = action_by_identifier(&store, "image:4:run");
    assert!(!mount
        .inputs
        .iter()
        .any(|path| path.ends_with("instructions/2/image")));
    let recipe = written(&store, "instructions/4/Dockerfile");
    assert!(recipe.contains("--mount=from=0,"), "{recipe}");
    let copy = action_by_identifier(&store, "image:5:copy");
    assert!(copy
        .inputs
        .iter()
        .any(|path| path.ends_with("instructions/2/image")));
}

#[test]
fn dockerfile_named_image_dependencies_that_declare_onbuild_select_whole_file_execution() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(
        workspace.path().join("Dockerfile"),
        "FROM parent\nCOPY a.txt /a\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("a.txt"), "alpha").unwrap();
    let source = format!(
        r#"{}
def _dockerfile_toolchain(ctx, auto_builder = False):
    return ("/tools/docker", "docker-test", "remote", "remote-builder")
def host_command(argv, merge_stderr = False, check = True):
    return ""
ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{"pull": False}},
    "deps_by_role": {{"images": [{{"label_id": "parent", "layout": ".once/out/parent/layout", "onbuild": True}}]}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".into(),
    );
    let (_, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let provider = result.unwrap();
    assert!(
        provider.contains("\"execution_mode\": \"buildkit\""),
        "{provider}"
    );
    assert!(provider.contains("\"onbuild\": True"), "{provider}");
}

#[test]
fn dockerfile_named_images_select_the_layout_entry_by_their_first_tag() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(
        workspace.path().join("Dockerfile"),
        "FROM base\nCOPY a.txt /a\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("a.txt"), "alpha").unwrap();
    let source = format!(
        r#"{}
def _dockerfile_toolchain(ctx, auto_builder = False):
    return ("/tools/docker", "docker-test", "remote", "remote-builder")
def host_command(argv, merge_stderr = False, check = True):
    return ""
ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{"execution_mode": "buildkit", "pull": False}},
    "deps_by_role": {{"images": [
        {{"label_id": "base", "layout": ".once/out/base/layout", "tags": ["base:v1", "base:v2"], "target_kind": "oci_image"}},
    ]}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".into(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let build = action_by_identifier(&store, "image:dockerfile-build");
    assert!(
        build
            .argv
            .iter()
            .any(|arg| arg.starts_with("base=oci-layout://")
                && arg.ends_with("/.once/out/base/layout:v1")),
        "{:?}",
        build.argv
    );
}

#[test]
fn dockerfile_instruction_steps_select_the_tagged_entry_of_a_named_image_base() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(workspace.path().join("Dockerfile"), "FROM base\nRUN true\n").unwrap();
    let source = format!(
        r#"{}
def _dockerfile_toolchain(ctx, auto_builder = False):
    return ("/tools/docker", "docker-test", "remote", "remote-builder")
def host_command(argv, merge_stderr = False, check = True):
    return ""
ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{"execution_mode": "instructions", "pull": False}},
    "deps_by_role": {{"images": [
        {{"label_id": "base", "layout": ".once/out/base/layout", "tags": ["base:v1", "base:v2"], "target_kind": "oci_image"}},
    ]}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".into(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let from = action_by_identifier(&store, "image:1:from");
    assert!(
        from.argv
            .iter()
            .any(|arg| arg == "once_previous=oci-layout://.once/out/base/layout:v1"),
        "{:?}",
        from.argv
    );
}

#[test]
fn dockerfile_copy_sources_resolve_the_stage_arguments_and_environment() {
    let (store, _) = analyze(
        "FROM scratch\nENV NAME=\"early\"\nARG SUFFIX=txt\nCOPY ${NAME}.$SUFFIX /early\nCOPY [\"$NAME.txt\", \"/json\"]\n",
        "build",
    );
    for identifier in ["image:4:copy", "image:5:copy"] {
        let action = action_by_identifier(&store, identifier);
        assert!(
            action.inputs.iter().any(|path| path == "early.txt"),
            "{identifier}"
        );
        assert!(
            !action.inputs.iter().any(|path| path == "late.txt"),
            "{identifier}"
        );
    }
}

#[test]
fn dockerfile_copy_sources_keep_the_full_context_when_a_variable_cannot_be_resolved() {
    let (store, _) = analyze(
        "FROM scratch\nARG UNSET\nENV LITERAL='$NAME' NAME=early\nCOPY $UNSET.txt /unset\nCOPY '$NAME.txt' /quoted\nCOPY $LITERAL.txt /literal\n",
        "build",
    );
    for identifier in ["image:4:copy", "image:5:copy", "image:6:copy"] {
        let action = action_by_identifier(&store, identifier);
        assert!(
            action.inputs.iter().any(|path| path == "early.txt"),
            "{identifier}"
        );
        assert!(
            action.inputs.iter().any(|path| path == "late.txt"),
            "{identifier}"
        );
    }
}

#[test]
fn dockerfile_copy_sources_keep_the_full_context_when_an_argument_and_environment_disagree() {
    let (store, _) = analyze(
        "FROM scratch\nARG NAME=late\nENV NAME=early\nCOPY $NAME.txt /picked\nENV OTHER=early\nARG OTHER=late\nCOPY $OTHER.txt /other\nARG ONLY=early\nCOPY $ONLY.txt /only\n",
        "build",
    );
    for identifier in ["image:4:copy", "image:7:copy"] {
        let ambiguous = action_by_identifier(&store, identifier);
        assert!(
            ambiguous.inputs.iter().any(|path| path == "early.txt"),
            "{identifier}"
        );
        assert!(
            ambiguous.inputs.iter().any(|path| path == "late.txt"),
            "{identifier}"
        );
    }
    let argument = action_by_identifier(&store, "image:9:copy");
    assert!(argument.inputs.iter().any(|path| path == "early.txt"));
    assert!(!argument.inputs.iter().any(|path| path == "late.txt"));
}

#[test]
fn dockerfile_instruction_snapshots_are_intermediate_and_the_export_is_not() {
    let (store, _) = analyze("FROM scratch\nCOPY early.txt /early\n", "build");
    assert!(action_by_identifier(&store, "image:1:from").intermediate);
    assert!(action_by_identifier(&store, "image:2:copy").intermediate);
    assert!(!action_by_identifier(&store, "image:dockerfile-export").intermediate);
}
