use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModuleAuthoringContract {
    pub language: &'static str,
    pub registration: &'static str,
    pub declaration_source: String,
    pub schema_invariants: Vec<&'static str>,
    pub context_fields: Vec<ContractEntry>,
    pub analysis_primitives: Vec<ContractEntry>,
    pub action_primitives: Vec<ContractEntry>,
    pub maintenance_invariants: Vec<&'static str>,
    pub starter: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContractEntry {
    pub signature: &'static str,
    pub purpose: &'static str,
}

#[must_use]
pub fn module_authoring_contract() -> ModuleAuthoringContract {
    let common = crate::modules::common_module_source();
    let declaration_source = common
        .split_once("\ndef _ends_with")
        .map_or(common, |(public, _)| public)
        .trim()
        .to_string();
    ModuleAuthoringContract {
        language: "Starlark",
        registration: "[modules]\npaths = [\"modules/*.star\"]\n",
        declaration_source,
        schema_invariants: vec![
            "Supported attribute types are string, bool, int, float, list<string>, map<string,string>, target, and select values for configurable attributes.",
            "attr.default is optional schema documentation and must be a string; it does not insert a runtime value. Implementations must use ctx[\"attr\"].get(...) when an optional attribute needs a fallback.",
            "Set configurable = False when analysis or output identity cannot safely vary through select.",
            "Dependency declarations name provider records accepted from ctx[\"deps\"], and implementations should consume provider fields instead of dependency target kind names.",
            "An implementation must return a JSON-shaped provider record whose fields satisfy the target kind's declared provider contract.",
        ],
        context_fields: vec![
            entry("ctx[\"label\"]", "Package, name, and stable target id."),
            entry("ctx[\"attr\"]", "Typed target attributes."),
            entry("ctx[\"srcs\"]", "Declared source patterns."),
            entry("ctx[\"deps\"]", "Provider records returned by dependencies."),
            entry("ctx[\"build_dir\"]", "Workspace-relative durable output directory."),
            entry("ctx[\"scratch_dir\"]", "Workspace-relative action-private directory."),
            entry("ctx[\"capability\"]", "Capability being analyzed."),
            entry("ctx[\"run\"][\"visible\"]", "Whether a visible runtime was requested."),
        ],
        analysis_primitives: vec![
            entry("glob(patterns)", "Expand package source patterns into sorted workspace paths."),
            entry("host_arch()", "Read the normalized host architecture."),
            entry("host_os()", "Read the normalized host operating system."),
            entry("host_env(name)", "Read one host environment variable."),
            entry("workspace_root()", "Read the absolute workspace root."),
            entry("host_which(name)", "Resolve a required executable."),
            entry("host_which_optional(name)", "Resolve an optional executable."),
            entry(
                "host_command(argv, env = {}, merge_stderr = False)",
                "Run a discovery command whose arguments and environment participate in analysis caching.",
            ),
            entry("host_file_exists(path)", "Test whether a host file exists."),
            entry("host_file_read(path)", "Read a host text file during analysis."),
            entry("host_file_sha256(path)", "Digest a host file used during analysis."),
            entry("host_file_contains(path, needle)", "Search a host text file."),
            entry("host_read_dir(path)", "List sorted names in a host directory."),
            entry("json_decode(source)", "Decode structured JSON data for a resolver."),
            entry("toml_decode(source)", "Decode structured TOML data for a resolver."),
        ],
        action_primitives: vec![
            entry("declare_output(name)", "Reserve a durable target output path."),
            entry("write_path(path, content)", "Declare a portable file-writing action."),
            entry(
                "copy_path(source, destination, kind = \"file\", inputs = [], toolchain_identity = None, identifier = None, cacheable = True)",
                "Declare a portable file or directory copy action.",
            ),
            entry(
                "materialize_host_file(source, destination)",
                "Snapshot a content-verified absolute host toolchain file into a workspace output.",
            ),
            entry(
                "prepare_path(path, kind, identifier = None)",
                "Declare uncached path removal or directory creation when standalone preparation is required.",
            ),
            entry(
                "write_tree_digest(root, output, include_suffixes = [], inputs = [], identifier = None, cacheable = True)",
                "Declare a deterministic workspace tree digest action.",
            ),
            entry(
                "cmd_args(args, use_arg_file = None)",
                "Build a structured argument list, optionally backed by an argument file.",
            ),
            entry(
                "run_action(argv, inputs = [], outputs = [], clean_paths = [], create_dirs = [], cwd = None, env = {}, toolchain_identity = None, identifier = None, cacheable = True, depends_on_prior_actions = True, stdout = None, stderr = None, sandbox = None)",
                "Declare a direct executable invocation with explicit inputs, outputs, setup, caching, and sandbox policy.",
            ),
        ],
        maintenance_invariants: vec![
            "Fetch and inspect the external rule or plugin that is authoritative for the requested behavior.",
            "Model only the requested target and its necessary dependency closure; leave unrelated nodes in the source build.",
            "Record the upstream system, symbol, web address, adoption intent, and the content digest of every complete fetched source with source_reference(...). Re-fetch and compare that digest before maintaining the adaptation.",
            "Keep ecosystem interpretation in the project module; use only generic Once primitives in the executor.",
            "Declare command arguments, inputs, outputs, cleanup, and directories explicitly instead of hiding setup in a shell command.",
            "Validate the module, validate target tables, validate the workspace, execute the requested capability, and inspect fresh evidence.",
        ],
        starter: r#"def _generated_text_impl(ctx):
    out = declare_output(ctx["attr"]["output"])
    write_path(out, "\n".join(ctx["attr"]["lines"]) + "\n")
    return {
        "label_id": ctx["label"]["id"],
        "generated_file": out,
    }

generated_text = target_kind(
    docs = "Writes declared lines to a generated text file.",
    attrs = [
        attr("output", "string", required = True, configurable = False),
        attr("lines", "list<string>", required = True),
    ],
    providers = ["generated_file"],
    capabilities = [capability("build", ["default"])],
    impl = _generated_text_impl,
)
"#,
    }
}

const fn entry(signature: &'static str, purpose: &'static str) -> ContractEntry {
    ContractEntry { signature, purpose }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_exposes_declarations_actions_and_maintenance_loop() {
        let contract = module_authoring_contract();
        assert!(contract.declaration_source.contains("def target_kind("));
        assert!(contract
            .declaration_source
            .contains("def source_reference("));
        assert!(contract
            .action_primitives
            .iter()
            .any(|entry| entry.signature.starts_with("run_action(")));
        assert!(contract
            .action_primitives
            .iter()
            .any(|entry| entry.signature.starts_with("materialize_host_file(")));
        assert!(contract
            .schema_invariants
            .iter()
            .any(|invariant| invariant.contains("attr.default")));
        assert!(contract
            .maintenance_invariants
            .iter()
            .any(|invariant| invariant.contains("dependency closure")));
    }
}
