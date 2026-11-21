//! Action declaration: outputs, deferred expansions, command lines, and `run_action`.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context};
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::dict::AllocDict;
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::Value;

use super::{
    apply_arg_format, unpack_action_argv, unpack_cmd_args_arg_file, unpack_i32_list,
    unpack_resources, validate_declared_arg_file_args, validate_network, validate_sandbox,
    CMD_ARGS_MARKER,
};
use crate::analysis::action_history::unpack_history;
use crate::analysis::presentation::{record_action, unpack_presentation, unpack_source_files};
use crate::analysis::store::{with_store_mut, DeclaredAction, DeclaredActionOperation};
use crate::analysis::transitive_set;
use crate::analysis::values::{unpack_string_dict, unpack_string_list};

#[starlark_module]
pub(super) fn action_globals(builder: &mut GlobalsBuilder) {
    /// A set of strings built from this target's `direct` items and the
    /// `transitive` sets of its dependencies, in time proportional to its own
    /// items. Entries of `transitive` may be sets, lists of strings, or `None`.
    /// Flatten it with `.to_list()`, or pass it to `run_action(inputs = ...)`
    /// or `cmd_args`, which retain shared references until consumed.
    fn transitive_set<'v>(
        #[starlark(require = named)] direct: Option<Value<'v>>,
        #[starlark(require = named)] transitive: Option<Value<'v>>,
    ) -> anyhow::Result<transitive_set::TransitiveSet> {
        transitive_set::build(direct, transitive)
    }

    /// Reserve a workspace-relative output path under the active
    /// target's build directory and return it. Outside analysis this
    /// returns the bare name.
    fn declare_output(name: &str) -> anyhow::Result<String> {
        with_store_mut(|store| match store {
            Some(store) => {
                let path = format!("{}/{}", store.build_dir, name);
                store.declared_outputs.push(path.clone());
                Ok(path)
            }
            None => Ok(name.to_string()),
        })
    }

    fn expand_actions<'v>(
        implementation: &str,
        inputs: Value<'v>,
        outputs: Value<'v>,
        args: Value<'v>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        let presentation = unpack_presentation(presentation);
        let inputs = unpack_string_list(inputs, "inputs")?;
        let outputs = unpack_string_list(outputs, "outputs")?;
        let args = args
            .to_json_value()
            .context("action planner arguments must be serializable data")?;
        if implementation.is_empty() {
            anyhow::bail!("expand_actions requires an implementation function name");
        }
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(DeclaredAction {
                    presentation,
                    history: None,
                    resources: struo_core::ResourceRequest::default(),
                    operation: Some(DeclaredActionOperation::ExpandActions {
                        implementation: implementation.to_string(),
                        args,
                        build_dir: store.build_dir.clone(),
                    }),
                    argv: Vec::new().into(),
                    arg_files: Vec::new(),
                    inputs: inputs.into(),
                    outputs,
                    stdout: None,
                    stderr: None,
                    clean_paths: Vec::new(),
                    create_dirs: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    sandbox: None,
                    network: None,
                    success_exit_codes: vec![0],
                    cacheable: true,
                    inherit_parent_env: false,
                    depends_on_prior_actions: true,
                    intermediate: false,
                    toolchain_identity: None,
                    identifier: Some(format!("expand_actions:{implementation}")),
                    display_name: None,
                    source_files: Vec::new(),
                });
            }
        });
        Ok(NoneType)
    }

    /// Build a structured command-line fragment. `args` is a transitive set
    /// or a list of strings and transitive sets. `format` rewrites each item,
    /// replacing its one `{}` with the item, and `before_each` adds a separate
    /// argument before each item. When `use_arg_file` is set, it must be a
    /// dict with `path` plus optional `format` and `arg_format`. The supported
    /// format is `"line-delimited"`.
    fn cmd_args<'v>(
        args: Value<'v>,
        use_arg_file: Option<Value<'v>>,
        #[starlark(require = named)] format: Option<String>,
        #[starlark(require = named)] before_each: Option<String>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let declared =
            crate::analysis::DeclaredArgs::unpack(args, format.clone(), before_each.clone())?;
        let arg_file = match use_arg_file {
            Some(value) => unpack_cmd_args_arg_file(value)?,
            None => None,
        };
        if let Some(arg_file) = &arg_file {
            validate_declared_arg_file_args(arg_file.format, &declared, &arg_file.path)?;
            apply_arg_format(&arg_file.arg_format, &arg_file.path)?;
        }
        let heap = eval.heap();
        let args = ListRef::from_value(args)
            .map_or(args, |list| heap.alloc(list.iter().collect::<Vec<_>>()));
        let mut pairs = vec![
            (CMD_ARGS_MARKER.to_string(), Value::new_bool(true)),
            ("args".to_string(), args),
        ];
        if let Some(format) = format {
            pairs.push(("format".to_string(), heap.alloc(format)));
        }
        if let Some(before_each) = before_each {
            pairs.push(("before_each".to_string(), heap.alloc(before_each)));
        }
        if let Some(arg_file) = arg_file {
            pairs.extend([
                ("arg_file_path".to_string(), heap.alloc(arg_file.path)),
                (
                    "arg_file_format".to_string(),
                    heap.alloc(arg_file.format.as_str().to_string()),
                ),
                ("arg_format".to_string(), heap.alloc(arg_file.arg_format)),
            ]);
        }
        Ok(heap.alloc(AllocDict(pairs)))
    }

    /// Record one command action declaration. Argument shape:
    /// `argv`: list of strings and `cmd_args` values; `inputs`: list
    /// of workspace-relative source paths to hash into the input
    /// digest; `outputs`: list of workspace-relative paths the action
    /// produces; `clean_paths`: optional list of workspace paths to
    /// remove before a fresh command execution; `create_dirs`: optional
    /// list of workspace directories to create before a fresh command
    /// execution; `cwd`: optional workspace-relative directory to run
    /// the command in, defaulting to the workspace root; `env`: optional
    /// string->string dict; `cacheable`: optional bool, default true;
    /// `inherit_parent_env`: optional bool for unsandboxed local run actions,
    /// default false; cacheable actions hash the effective environment;
    /// `sandbox`: optional local filesystem sandbox policy, `"off"`,
    /// `"inputs"`, or `"copied-inputs"`; the copied mode materializes
    /// private input copies for tools that cannot consume links;
    /// `success_exit_codes`: optional integer list, default `[0]`, whose
    /// members mean the command completed and its outputs are valid;
    /// `toolchain_identity`: optional string folded into the input
    /// digest; `identifier`: optional label for diagnostics;
    /// `intermediate`: optional bool, default false, for outputs only later
    /// actions of the same target read, so a cache hit doesn't restore them
    /// to disk unless one of those actions has to run.
    #[allow(
        clippy::too_many_arguments,
        reason = "run_action mirrors the declared-action fields, including optional stream redirection"
    )]
    fn run_action<'v>(
        argv: Value<'v>,
        inputs: Option<Value<'v>>,
        outputs: Option<Value<'v>>,
        clean_paths: Option<Value<'v>>,
        create_dirs: Option<Value<'v>>,
        cwd: Option<Value<'v>>,
        env: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
        inherit_parent_env: Option<bool>,
        depends_on_prior_actions: Option<bool>,
        stdout: Option<String>,
        stderr: Option<String>,
        sandbox: Option<String>,
        network: Option<String>,
        success_exit_codes: Option<Value<'v>>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] history: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
        #[starlark(require = named)] intermediate: Option<bool>,
        #[starlark(require = named)] resources: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        validate_sandbox(sandbox.as_deref())?;
        validate_network(network.as_deref())?;
        let resources = unpack_resources(resources)?;
        let argv = unpack_action_argv(argv, "argv")?;
        let inputs = inputs
            .map(|value| super::super::DeclaredInputs::unpack(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let outputs = outputs
            .map(|value| unpack_string_list(value, "outputs"))
            .transpose()?
            .unwrap_or_default();
        let clean_paths = clean_paths
            .map(|value| unpack_string_list(value, "clean_paths"))
            .transpose()?
            .unwrap_or_default();
        let create_dirs = create_dirs
            .map(|value| unpack_string_list(value, "create_dirs"))
            .transpose()?
            .unwrap_or_default();
        let cwd = match cwd {
            None => None,
            Some(value) if value.is_none() => None,
            Some(value) => Some(
                value
                    .unpack_str()
                    .ok_or_else(|| anyhow::anyhow!("cwd must be a string or None"))?
                    .to_string(),
            ),
        };
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        let cacheable = cacheable.unwrap_or(true);
        let inherit_parent_env = inherit_parent_env.unwrap_or(false);
        if inherit_parent_env && sandbox.as_deref().is_some_and(|mode| mode != "off") {
            return Err(anyhow!(
                "run_action inherit_parent_env requires sandbox = \"off\""
            ));
        }
        let mut success_exit_codes = success_exit_codes
            .map(|value| unpack_i32_list(value, "success_exit_codes"))
            .transpose()?
            .unwrap_or_else(|| vec![0]);
        if success_exit_codes.is_empty() {
            anyhow::bail!("success_exit_codes must contain at least one exit code");
        }
        success_exit_codes.sort_unstable();
        success_exit_codes.dedup();
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: unpack_history(history),
            resources,
            operation: None,
            argv: argv.args,
            arg_files: argv.arg_files,
            inputs,
            outputs,
            stdout,
            stderr,
            clean_paths,
            create_dirs,
            cwd,
            env,
            sandbox,
            network,
            success_exit_codes,
            cacheable,
            inherit_parent_env,
            depends_on_prior_actions: depends_on_prior_actions.unwrap_or(true),
            intermediate: intermediate.unwrap_or(false),
            toolchain_identity,
            identifier,
            display_name,
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }
}
