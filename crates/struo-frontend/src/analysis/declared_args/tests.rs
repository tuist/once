use std::path::Path;

use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

use super::DeclaredArgs;
use crate::analysis::{with_active_store, AnalysisStore, DeclaredAction};

fn evaluate(workspace: &Path, source: &str) -> anyhow::Result<Vec<DeclaredAction>> {
    let store = AnalysisStore::new(
        workspace.to_path_buf(),
        "pkg".into(),
        ".struo/out/pkg".into(),
    );
    let (store, result) = with_active_store(store, || {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("args.star", source.to_owned(), &Dialect::Standard)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            Evaluator::new(&module)
                .eval_module(ast, &crate::analysis::globals_for_prelude())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok::<_, anyhow::Error>(())
        })
    });
    result?;
    Ok(store.actions)
}

#[test]
fn declaration_hashing_serialization_and_argfile_validation_do_not_materialize_sets() {
    let workspace = tempfile::tempdir().unwrap();
    let actions = evaluate(
        workspace.path(),
        r#"
leaf = transitive_set(direct = ["leaf{}".format(i) for i in range(512)])
left = transitive_set(direct = ["left", ""], transitive = [leaf])
right = transitive_set(direct = ["right"], transitive = [leaf])
root = transitive_set(direct = ["root"], transitive = [left, right])
run_action(argv = [
    "tool", "",
    cmd_args([root, "duplicate", "duplicate", root], format = "<{}>", before_each = "-I"),
    cmd_args(root, format = "--{}", before_each = "", use_arg_file = {"path": "args.txt"}),
])
"#,
    )
    .unwrap();
    let action = &actions[0];
    assert!(action.argv.flattened.get().is_none());
    assert!(action.arg_files[0].args.flattened.get().is_none());
    let digest = action.argv.structural_digest();
    let file_digest = action.arg_files[0].args.structural_digest();
    assert!(action.argv.flattened.get().is_none());
    let json = serde_json::to_value(action).unwrap();
    let encoded = serde_json::to_string(&json).unwrap();
    assert!(
        encoded.len() < 1500,
        "declaration grew with closure: {}",
        encoded.len()
    );
    assert!(!encoded.contains("leaf511"));
    assert_eq!(json["argv"][2]["format"], "<{}>");
    assert_eq!(json["argv"][2]["before_each"], "-I");
    assert!(action.argv.flattened.get().is_none());
    assert!(action.arg_files[0].args.flattened.get().is_none());

    let mut decoded: DeclaredAction = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(decoded.argv.structural_digest(), digest);
    assert_eq!(decoded.arg_files[0].args.structural_digest(), file_digest);
    assert_eq!(serde_json::to_value(&decoded).unwrap(), json);
    decoded.resolve_args(workspace.path()).unwrap();
    assert!(decoded.argv.flattened.get().is_none());
    assert!(decoded.arg_files[0].args.flattened.get().is_none());
    assert_eq!(decoded.argv.structural_digest(), digest);
    assert_eq!(decoded.argv.to_vec(), action.argv.to_vec());
    assert_eq!(
        decoded.arg_files[0].args.to_vec(),
        action.arg_files[0].args.to_vec()
    );
    assert_eq!(decoded.argv.len(), decoded.argv.iter().count());
    let args = decoded.argv.to_vec();
    assert_eq!(
        &args[..8],
        ["tool", "", "-I", "<root>", "-I", "<left>", "-I", "<>"]
    );
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "<duplicate>")
            .count(),
        2
    );
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "<leaf511>")
            .count(),
        2
    );
    assert_eq!(args.last().unwrap(), "@args.txt");
    assert_eq!(
        &decoded.arg_files[0].args.to_vec()[..4],
        ["", "--root", "", "--left"]
    );
    assert_eq!(decoded.argv.structural_digest(), digest);
    assert_eq!(serde_json::to_value(&decoded).unwrap(), json);

    let missing = tempfile::tempdir().unwrap();
    let mut decoded: DeclaredAction = serde_json::from_value(json.clone()).unwrap();
    assert!(decoded.resolve_args(missing.path()).is_err());
    let mut file_only: DeclaredAction = serde_json::from_value(json).unwrap();
    file_only.argv = vec![String::from("tool")].into();
    assert!(file_only.resolve_args(missing.path()).is_err());
}

#[test]
fn structural_digests_distinguish_boundaries_options_and_topology() {
    let workspace = tempfile::tempdir().unwrap();
    let actions = evaluate(
        workspace.path(),
        r#"
a = transitive_set(direct = ["a", "b"])
b = transitive_set(direct = ["a"], transitive = [["b"]])
run_action(argv = ["ab", "c"])
run_action(argv = ["a", "bc"])
run_action(argv = [""])
run_action(argv = [])
run_action(argv = ["a", ""])
run_action(argv = ["a"])
run_action(argv = ["b", "a"])
run_action(argv = ["a", "b"])
run_action(argv = [cmd_args(a)])
run_action(argv = [cmd_args(b)])
run_action(argv = [cmd_args(a, format = "{}")])
run_action(argv = [cmd_args(a, format = "--{}")])
run_action(argv = [cmd_args(a, before_each = "")])
run_action(argv = [cmd_args(a, before_each = "-I")])
run_action(argv = [cmd_args(a), cmd_args(a)])
"#,
    )
    .unwrap();
    let digests = actions
        .iter()
        .map(|action| action.argv.structural_digest())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(digests.len(), actions.len());
    assert!(actions
        .iter()
        .all(|action| action.argv.flattened.get().is_none()));
    assert_eq!(actions[8].argv.to_vec(), actions[9].argv.to_vec());
    for action in actions {
        let digest = action.argv.structural_digest();
        let json = serde_json::to_value(&action.argv).unwrap();
        let decoded: DeclaredArgs = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.structural_digest(), digest);
    }
}

#[test]
fn replay_shares_nodes_between_argv_and_response_files() {
    let workspace = tempfile::tempdir().unwrap();
    let actions = evaluate(
        workspace.path(),
        r#"
paths = transitive_set(direct = ["a", "b"])
run_action(argv = [cmd_args(paths), cmd_args(paths, use_arg_file = {"path": "args.txt"})])
"#,
    )
    .unwrap();
    let mut replayed: DeclaredAction =
        serde_json::from_value(serde_json::to_value(&actions[0]).unwrap()).unwrap();
    replayed.resolve_args(workspace.path()).unwrap();
    let super::Entry::Fragment {
        args: argv_inputs, ..
    } = &replayed.argv.entries[0]
    else {
        panic!("expected argv set fragment")
    };
    let super::Entry::Fragment {
        args: file_inputs, ..
    } = &replayed.arg_files[0].args.entries[0]
    else {
        panic!("expected argfile set fragment")
    };
    let crate::analysis::InputFragment::Set(argv_node) = argv_inputs.fragments().next().unwrap()
    else {
        panic!("expected argv root")
    };
    let crate::analysis::InputFragment::Set(file_node) = file_inputs.fragments().next().unwrap()
    else {
        panic!("expected argfile root")
    };
    assert!(std::ptr::eq(argv_node, file_node));
}

#[test]
fn legacy_arrays_and_list_snapshot_preserve_empty_strings_and_repetition() {
    let expected = vec![String::from("a"), String::new(), String::from("a")];
    let args = DeclaredArgs::from(expected.clone());
    assert_eq!(args, expected);
    assert_eq!((&args).into_iter().cloned().collect::<Vec<_>>(), expected);
    assert_eq!(expected.into_iter().collect::<DeclaredArgs>(), args);
    let json = serde_json::to_value(&args).unwrap();
    assert_eq!(json, serde_json::json!(["a", "", "a"]));
    assert_eq!(serde_json::from_value::<DeclaredArgs>(json).unwrap(), args);
    assert!(DeclaredArgs::default().is_empty());

    let workspace = tempfile::tempdir().unwrap();
    let actions = evaluate(
        workspace.path(),
        r#"
raw = ["a", "a", ""]
fragment = cmd_args(raw, format = "<{}>", before_each = "")
raw.append("late")
run_action(argv = [fragment, fragment])
"#,
    )
    .unwrap();
    assert_eq!(
        actions[0].argv,
        ["", "<a>", "", "<a>", "", "<>", "", "<a>", "", "<a>", "", "<>"]
    );
}

#[test]
fn argfile_newline_and_format_diagnostics_remain_exact_without_flattening() {
    let workspace = tempfile::tempdir().unwrap();
    for args in [
        r#"cmd_args(transitive_set(transitive = [["bad\narg"]]), use_arg_file = {"path": "args.txt"})"#,
        r#"cmd_args(["a"], format = "{}\n", use_arg_file = {"path": "args.txt"})"#,
        r#"cmd_args(["a"], before_each = "\r", use_arg_file = {"path": "args.txt"})"#,
    ] {
        let error =
            evaluate(workspace.path(), &format!("run_action(argv = [{args}])\n")).unwrap_err();
        assert!(error
            .to_string()
            .contains("line-delimited arg file `args.txt` contains an argument with a newline"));
    }
    let actions = evaluate(workspace.path(), r#"
run_action(argv = [cmd_args(transitive_set(), format = "{}\n", before_each = "\r", use_arg_file = {"path": "empty.txt"})])
"#).unwrap();
    assert!(actions[0].arg_files[0].args.is_empty());
    assert!(actions[0].arg_files[0].args.flattened.get().is_none());
    for format in ["", "fixed", "{}{}"] {
        let error = evaluate(
            workspace.path(),
            &format!("cmd_args([], format = {format:?})\n"),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("expected `cmd_args.format` to contain exactly one `{}` placeholder"));
    }
}

#[test]
fn prepending_and_cloning_preserve_lazy_fragments_and_discard_materialized_copies() {
    let workspace = tempfile::tempdir().unwrap();
    let actions = evaluate(
        workspace.path(),
        r#"
run_action(argv = [cmd_args(transitive_set(direct = ["a", "b"]), format = "--{}")])
"#,
    )
    .unwrap();
    let original = &actions[0].argv;
    assert_eq!(original[0], "--a");
    assert!(original.flattened.get().is_some());
    let mut cloned = original.clone();
    assert!(cloned.flattened.get().is_none());
    cloned.prepend(&["wrapper".into(), String::new()]);
    assert!(cloned.flattened.get().is_none());
    assert_ne!(cloned.structural_digest(), original.structural_digest());
    let json = serde_json::to_value(&cloned).unwrap();
    assert_eq!(json[0], "wrapper");
    assert_eq!(json[1], "");
    assert!(json[2]["args"][0].get("$struo_transitive_set").is_some());
    assert_eq!(cloned.to_vec(), ["wrapper", "", "--a", "--b"]);
    assert!(cloned.flattened.get().is_none());
    assert_eq!(cloned[0], "wrapper");
    cloned.prepend(&["outer".into()]);
    assert!(cloned.flattened.get().is_none());
    assert_eq!(cloned.to_vec(), ["outer", "wrapper", "", "--a", "--b"]);

    let mut unresolved: DeclaredArgs = serde_json::from_value(json).unwrap();
    unresolved.prepend(&["outer".into()]);
    assert!(unresolved.flattened.get().is_none());
    assert_eq!(unresolved.structural_digest(), cloned.structural_digest());
    unresolved.resolve(workspace.path()).unwrap();
    assert_eq!(unresolved.to_vec(), cloned.to_vec());
}
