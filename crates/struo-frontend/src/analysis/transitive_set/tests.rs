use std::sync::Arc;

use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

use super::{transitive_sets_available, Node, TransitiveSets};
use crate::analysis::store::{with_active_store, AnalysisStore, DeclaredAction};
use crate::analysis::values::value_to_json;

/// What target kinds compute today: own items, then each dependency's
/// flattened list, without duplicates.
fn collect_like_the_prelude(own: &[String], deps: &[Vec<String>]) -> Vec<String> {
    let mut out = Vec::new();
    for item in own.iter().chain(deps.iter().flatten()) {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
    out
}

/// Random graphs with repeated items, empty sets, diamonds, repeated
/// children and list leaves: every shape the normalizing builder rewrites.
#[test]
fn flattening_matches_collecting_lists_on_random_graphs() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move |bound: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % bound
    };
    for _ in 0..2_000 {
        let mut nodes: Vec<Arc<Node>> = Vec::new();
        let mut lists: Vec<Vec<String>> = Vec::new();
        for _ in 0..=next(12) {
            let own = (0..next(4))
                .map(|_| format!("item{}", next(10)))
                .collect::<Vec<_>>();
            let mut children = Vec::new();
            let mut child_lists = Vec::new();
            for _ in 0..next(4) {
                if nodes.is_empty() || next(5) == 0 {
                    let leaf = (0..next(3))
                        .map(|_| format!("leaf{}", next(6)))
                        .collect::<Vec<_>>();
                    children.push(Node::build(leaf.clone(), Vec::new()));
                    child_lists.push(collect_like_the_prelude(&leaf, &[]));
                } else {
                    let index = usize::try_from(next(nodes.len() as u64)).unwrap();
                    children.push(Arc::clone(&nodes[index]));
                    child_lists.push(lists[index].clone());
                }
            }
            lists.push(collect_like_the_prelude(&own, &child_lists));
            nodes.push(Node::build(own, children));
        }
        for (node, expected) in nodes.iter().zip(&lists) {
            assert_eq!(&node.to_vec(), expected);
        }
    }
}

#[test]
fn digests_name_contents_and_child_order() {
    let leaf =
        |items: &[&str]| Node::build(items.iter().map(ToString::to_string).collect(), Vec::new());
    let left = || leaf(&["a"]);
    let right = || leaf(&["b"]);
    let both = Node::build(vec!["x".into()], vec![left(), right()]);
    assert_eq!(
        both.digest,
        Node::build(vec!["x".into()], vec![left(), right()]).digest
    );
    assert_ne!(
        both.digest,
        Node::build(vec!["x".into()], vec![right(), left()]).digest
    );
    let empty = Node::build(Vec::new(), Vec::new());
    assert!(Arc::ptr_eq(
        &Node::build(Vec::new(), vec![Arc::clone(&both), empty]),
        &both
    ));
}

#[test]
fn exported_sets_resolve_in_a_later_session_and_damaged_ones_do_not() {
    let workspace = tempfile::tempdir().unwrap();
    let shared = Node::build(vec!["shared".into()], Vec::new());
    let middle = Node::build(vec!["middle".into()], vec![Arc::clone(&shared)]);
    let top = Node::build(vec!["top".into()], vec![middle, Arc::clone(&shared)]);
    TransitiveSets::default().export(workspace.path(), &top);

    let reference = serde_json::json!({"deps": [{super::REFERENCE_KEY: top.hex()}]});
    assert!(transitive_sets_available(workspace.path(), &reference));
    let resolved = TransitiveSets::default()
        .resolve(workspace.path(), &top.hex())
        .unwrap();
    assert_eq!(resolved.to_vec(), ["top", "middle", "shared"]);

    let shared_path = super::node_path(workspace.path(), &shared.hex());
    std::fs::write(&shared_path, br#"{"direct":["tampered"],"children":[]}"#).unwrap();
    assert!(!transitive_sets_available(workspace.path(), &reference));
    assert!(TransitiveSets::default()
        .resolve(workspace.path(), &top.hex())
        .is_err());
    TransitiveSets::default().export(workspace.path(), &top);
    assert!(transitive_sets_available(workspace.path(), &reference));
    std::fs::remove_file(&shared_path).unwrap();
    assert!(!transitive_sets_available(workspace.path(), &reference));
    TransitiveSets::default().export(workspace.path(), &top);
    assert!(transitive_sets_available(workspace.path(), &reference));
    std::fs::remove_file(super::node_path(workspace.path(), &top.hex())).unwrap();
    assert!(!transitive_sets_available(workspace.path(), &reference));
}

#[test]
fn missing_children_and_cycles_fail_before_node_authentication() {
    let workspace = tempfile::tempdir().unwrap();
    let a = "11".repeat(32);
    let b = "22".repeat(32);
    let path = super::node_path(workspace.path(), &a);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::json!({"direct": [], "children": [b]}).to_string(),
    )
    .unwrap();
    assert!(TransitiveSets::default()
        .resolve(workspace.path(), &a)
        .unwrap_err()
        .to_string()
        .contains("not stored"));

    std::fs::write(
        &path,
        serde_json::json!({"direct": [], "children": [a]}).to_string(),
    )
    .unwrap();
    assert!(TransitiveSets::default()
        .resolve(workspace.path(), &a)
        .unwrap_err()
        .to_string()
        .contains("cycle"));

    let child_path = super::node_path(workspace.path(), &b);
    std::fs::create_dir_all(child_path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::json!({"direct": [], "children": [b]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        child_path,
        serde_json::json!({"direct": [], "children": [a]}).to_string(),
    )
    .unwrap();
    assert!(TransitiveSets::default()
        .resolve(workspace.path(), &a)
        .unwrap_err()
        .to_string()
        .contains("cycle"));

    let shared = Node::build(vec!["shared".into()], Vec::new());
    let left = Node::build(vec!["left".into()], vec![Arc::clone(&shared)]);
    let right = Node::build(vec!["right".into()], vec![shared]);
    let root = Node::build(Vec::new(), vec![left, right]);
    TransitiveSets::default().export(workspace.path(), &root);
    assert_eq!(
        TransitiveSets::default()
            .resolve(workspace.path(), &root.hex())
            .unwrap()
            .to_vec(),
        ["left", "shared", "right"]
    );
}

fn evaluate(
    workspace: &std::path::Path,
    source: &str,
) -> (AnalysisStore, anyhow::Result<serde_json::Value>) {
    let store = AnalysisStore::new(
        workspace.to_path_buf(),
        "pkg".to_string(),
        ".struo/out/pkg".to_string(),
    );
    with_active_store(store, || {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("sets.star", source.to_string(), &Dialect::Standard)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            Evaluator::new(&module)
                .eval_module(ast, &crate::analysis::globals_for_prelude())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok(module
                .get("result")
                .map_or(serde_json::Value::Null, value_to_json))
        })
    })
}

fn only_action(store: &AnalysisStore) -> &DeclaredAction {
    assert_eq!(store.actions.len(), 1);
    &store.actions[0]
}

#[test]
fn topological_lists_keep_shared_children_after_their_parents() {
    let workspace = tempfile::tempdir().unwrap();
    let (_, result) = evaluate(
        workspace.path(),
        r#"
base = transitive_set(direct = ["base", "argument"])
left = transitive_set(direct = ["left"], transitive = [base])
right = transitive_set(direct = ["right"], transitive = [base])
root = transitive_set(direct = ["top"], transitive = [left, right, base])
result = [root.to_list(), root.to_list(order = "topological")]
"#,
    );
    assert_eq!(
        result.unwrap(),
        serde_json::json!([
            ["top", "left", "base", "argument", "right"],
            ["top", "left", "right", "base", "argument"],
        ])
    );
    let (_, result) = evaluate(
        workspace.path(),
        "transitive_set().to_list(order = 'unknown')",
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("preorder` or `topological"));
}

#[test]
fn actions_receive_flattened_sets() {
    let workspace = tempfile::tempdir().unwrap();
    let (store, result) = evaluate(
        workspace.path(),
        r#"
dep = transitive_set(direct = ["dep/a.h", "dep"], transitive = [["base/b.h"], None])
own = transitive_set(direct = ["own.h", "dep"], transitive = [dep])
run_action(
    argv = ["cc", cmd_args(own, format = "-D{}"), cmd_args(dep, before_each = "-I")],
    inputs = ["main.c", own],
    outputs = ["main.o"],
)
result = {"listed": own.to_list(), "empty": bool(transitive_set()), "set": own}
"#,
    );
    let result = result.unwrap();
    let action = only_action(&store);
    assert_eq!(
        action.argv,
        [
            "cc",
            "-Down.h",
            "-Ddep",
            "-Ddep/a.h",
            "-Dbase/b.h",
            "-I",
            "dep/a.h",
            "-I",
            "dep",
            "-I",
            "base/b.h"
        ]
    );
    assert_eq!(
        action.inputs,
        ["main.c", "own.h", "dep", "dep/a.h", "base/b.h"]
    );
    assert_eq!(
        result["listed"],
        serde_json::json!(["own.h", "dep", "dep/a.h", "base/b.h"])
    );
    assert_eq!(result["empty"], serde_json::json!(false));
    assert!(transitive_sets_available(workspace.path(), &result["set"]));
}

#[test]
fn list_operations_on_a_set_point_at_to_list() {
    let workspace = tempfile::tempdir().unwrap();
    for operation in ["[x for x in s]", "len(s)", "'a' in s", "s + []", "[] + s"] {
        let (_, result) = evaluate(
            workspace.path(),
            &format!("s = transitive_set(direct = ['a'])\nresult = {operation}\n"),
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("to_list()"), "{operation}: {error}");
    }
}

#[test]
fn declared_inputs_retain_shared_nodes_and_resolve_compact_roundtrips() {
    use crate::analysis::InputFragment;

    let workspace = tempfile::tempdir().unwrap();
    let (store, result) = evaluate(
        workspace.path(),
        r#"
leaf = transitive_set(direct = ["leaf", "shared"])
left = transitive_set(direct = ["left", "shared"], transitive = [leaf])
right = transitive_set(direct = ["right"], transitive = [leaf])
root = transitive_set(direct = ["root"], transitive = [left, right])
run_action(argv = ["tool"], inputs = ["same", "same", root, "leaf", root])
"#,
    );
    result.unwrap();
    let action = only_action(&store);
    let expected = [
        "same", "same", "root", "left", "shared", "leaf", "right", "leaf", "root", "left",
        "shared", "leaf", "right",
    ];
    assert_eq!(action.inputs, expected);
    assert_eq!(action.inputs.len(), expected.len());
    assert!(!action.inputs.is_empty());
    assert_eq!(action.inputs.iter().cloned().collect::<Vec<_>>(), expected);
    assert_eq!(
        (&action.inputs).into_iter().cloned().collect::<Vec<_>>(),
        expected
    );

    let fragments = action.inputs.fragments().collect::<Vec<_>>();
    assert_eq!(fragments.len(), 5);
    let InputFragment::Set(root) = fragments[2] else {
        panic!("expected set")
    };
    let InputFragment::Set(repeated) = fragments[4] else {
        panic!("expected set")
    };
    assert!(std::ptr::eq(root, repeated));
    assert_eq!(root.direct(), ["root"]);
    assert_eq!(root.children().len(), 2);
    assert_eq!(root.digest(), repeated.digest());

    let json = serde_json::to_value(action).unwrap();
    assert_eq!(
        json["inputs"],
        serde_json::json!([
            "same", "same", {super::REFERENCE_KEY: hex::encode(root.digest())},
            "leaf", {super::REFERENCE_KEY: hex::encode(root.digest())},
        ])
    );
    let mut decoded: DeclaredAction = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), json);
    drop(store);
    decoded.resolve_inputs(workspace.path()).unwrap();
    assert_eq!(decoded.inputs, expected);
    assert_eq!(decoded.inputs.to_vec(), expected);
    let fragments = decoded.inputs.fragments().collect::<Vec<_>>();
    let (InputFragment::Set(a), InputFragment::Set(b)) = (fragments[2], fragments[4]) else {
        panic!("expected repeated nodes")
    };
    assert!(std::ptr::eq(a, b));

    let missing = tempfile::tempdir().unwrap();
    let mut decoded: DeclaredAction = serde_json::from_value(json).unwrap();
    assert!(decoded
        .resolve_inputs(missing.path())
        .unwrap_err()
        .to_string()
        .contains("not stored"));
}

#[test]
fn declared_inputs_keep_legacy_arrays_and_empty_sets() {
    use crate::analysis::DeclaredInputs;

    let expected = vec![String::from("a"), String::from("a"), String::new()];
    let inputs = DeclaredInputs::from(expected.clone());
    assert_eq!(inputs, expected);
    assert_eq!(
        serde_json::to_value(&inputs).unwrap(),
        serde_json::json!(["a", "a", ""])
    );
    let mut decoded: DeclaredInputs =
        serde_json::from_value(serde_json::json!(["a", "a", ""])).unwrap();
    decoded.resolve(std::path::Path::new("unused")).unwrap();
    assert_eq!(decoded.to_vec(), expected);
    assert_eq!(expected.into_iter().collect::<DeclaredInputs>(), inputs);
    assert!(DeclaredInputs::default().is_empty());

    let workspace = tempfile::tempdir().unwrap();
    let (store, result) = evaluate(
        workspace.path(),
        "run_action(argv = ['tool'], inputs = transitive_set())\n",
    );
    result.unwrap();
    assert!(only_action(&store).inputs.is_empty());
    assert_eq!(only_action(&store).inputs.len(), 0);
}

#[test]
fn declared_input_concat_keeps_unresolved_references_and_entry_order() {
    use crate::analysis::DeclaredInputs;

    let reference = serde_json::json!({super::REFERENCE_KEY: "11".repeat(32)});
    let unresolved: DeclaredInputs =
        serde_json::from_value(serde_json::json!([reference])).unwrap();
    let combined = DeclaredInputs::concat([
        vec![String::from("a"), String::from("a")].into(),
        unresolved.clone(),
        DeclaredInputs::default(),
        unresolved,
        vec![String::from("last")].into(),
    ]);
    assert_eq!(
        serde_json::to_value(combined).unwrap(),
        serde_json::json!(["a", "a", reference, reference, "last"])
    );
    assert!(DeclaredInputs::concat([]).is_empty());
}

/// Write a workspace of C libraries, each `(index, deps, linkopts)`, and
/// analyse them in order, returning each library's provider.
fn analyze_c_libraries(
    libraries: &[(usize, Vec<usize>, Vec<&str>)],
) -> (
    tempfile::TempDir,
    Vec<serde_json::Value>,
    Vec<Vec<DeclaredAction>>,
) {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("struo.toml"),
        "[workspace]\ninclude = [\"struo.toml\", \"libs/*/struo.toml\"]\n",
    )
    .unwrap();
    for (index, deps, linkopts) in libraries {
        let package = workspace.path().join(format!("libs/l{index}"));
        std::fs::create_dir_all(package.join("include")).unwrap();
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(package.join(format!("include/l{index}.h")), "").unwrap();
        std::fs::write(package.join(format!("src/l{index}.c")), "").unwrap();
        let deps = deps
            .iter()
            .map(|dep| format!("\"libs/l{dep}/l{dep}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            package.join("struo.toml"),
            format!(
                "[[target]]\nname = \"l{index}\"\nkind = \"c_library\"\nsrcs = [\"src/*.c\"]\ndeps = [{deps}]\n\n\
                 [target.attrs]\nhdrs = [\"include/l{index}.h\"]\ndefines = [\"L{index}=1\"]\nlinkopts = {}\n",
                serde_json::to_string(linkopts).unwrap()
            ),
        )
        .unwrap();
    }
    let graph = crate::load_graph_workspace(workspace.path()).unwrap();
    let engine = crate::analysis::AnalysisEngine::for_workspace(workspace.path()).unwrap();
    let mut providers: Vec<serde_json::Value> = Vec::new();
    let mut actions = Vec::new();
    for (index, deps, _) in libraries {
        let id = format!("libs/l{index}/l{index}");
        let target = graph.iter().find(|target| target.label.id == id).unwrap();
        let deps = deps
            .iter()
            .map(|dep| providers[*dep].clone())
            .collect::<Vec<_>>();
        let result = engine
            .analyze_target(target, workspace.path(), &deps)
            .unwrap();
        providers.push(result.provider);
        actions.push(result.actions);
    }
    (workspace, providers, actions)
}

fn is_compile(action: &DeclaredAction) -> bool {
    action
        .identifier
        .as_deref()
        .is_some_and(|identifier| identifier.contains(":c-compile:"))
}

/// A provider holds references to its dependencies' sets, so its size does not
/// grow with the depth of the graph under it, while each compile still
/// receives every header in the closure, in the order the lists gave.
#[test]
fn c_providers_stay_the_same_size_down_a_chain() {
    const DEPTH: usize = 30;
    let libraries = (0..DEPTH)
        .map(|index| {
            (
                index,
                index.checked_sub(1).into_iter().collect(),
                Vec::new(),
            )
        })
        .collect::<Vec<_>>();
    let (workspace, providers, actions) = analyze_c_libraries(&libraries);
    let sizes = providers
        .iter()
        .map(|provider| serde_json::to_vec(provider).unwrap().len())
        .collect::<Vec<_>>();
    assert!(
        sizes[DEPTH - 1] <= sizes[2] + 64,
        "provider sizes grew with depth: {sizes:?}"
    );

    let top = DEPTH - 1;
    let expected = std::iter::once(format!("libs/l{top}/src/l{top}.c"))
        .chain(
            (0..DEPTH)
                .rev()
                .map(|index| format!("libs/l{index}/include/l{index}.h")),
        )
        .collect::<Vec<_>>();
    let compile = actions[top]
        .iter()
        .find(|action| is_compile(action))
        .unwrap();
    assert_eq!(compile.inputs, expected);

    // A later invocation reads the dependency's sets back from disk.
    let graph = crate::load_graph_workspace(workspace.path()).unwrap();
    let fresh = crate::analysis::AnalysisEngine::for_workspace(workspace.path()).unwrap();
    let top_target = graph
        .iter()
        .find(|target| target.label.id == format!("libs/l{top}/l{top}"))
        .unwrap();
    let rebuilt = fresh
        .analyze_target(top_target, workspace.path(), &[providers[top - 1].clone()])
        .unwrap();
    let compile = rebuilt
        .actions
        .iter()
        .find(|action| is_compile(action))
        .unwrap();
    assert_eq!(compile.inputs, expected);
}

/// Each library's link options arrive once however many paths reach it, and
/// stay whole: two libraries with the same options both keep theirs.
#[test]
fn c_link_options_arrive_once_per_library() {
    const DEPTH: usize = 30;
    let (_workspace, providers, _) = analyze_c_libraries(&[
        (0, vec![], vec!["-lshared"]),
        (1, vec![0], vec!["-framework", "Same"]),
        (2, vec![0], vec!["-framework", "Same"]),
        (3, vec![1, 2], vec!["-ltop"]),
    ]);
    assert_eq!(
        providers[3]["transitive_linkopts"],
        serde_json::json!([
            "-ltop",
            "-framework",
            "Same",
            "-framework",
            "Same",
            "-lshared"
        ])
    );

    // Each library depends on the two before it, so the number of paths to
    // the first one grows like the Fibonacci numbers while its options appear
    // once.
    let ladder = (0..DEPTH)
        .map(|index| {
            let deps = [index.checked_sub(1), index.checked_sub(2)]
                .into_iter()
                .flatten()
                .collect();
            (index, deps, vec!["-lm"])
        })
        .collect::<Vec<_>>();
    let (_workspace, providers, _) = analyze_c_libraries(&ladder);
    assert_eq!(
        providers[DEPTH - 1]["transitive_linkopts"]
            .as_array()
            .unwrap()
            .len(),
        DEPTH
    );
}

#[cfg(unix)]
#[test]
fn c_link_options_link_a_shared_static_dependency_after_both_dependents() {
    use std::process::Command;

    let (workspace, providers, _) = analyze_c_libraries(&[
        (0, vec![], vec!["-lbase"]),
        (1, vec![0], vec!["-lleft"]),
        (2, vec![0], vec!["-lright"]),
        (3, vec![1, 2], vec![]),
    ]);
    for (name, source) in [
        ("base_left", "int base_left(void) { return 3; }"),
        ("base_right", "int base_right(void) { return 7; }"),
        (
            "left",
            "int base_left(void); int left(void) { return base_left(); }",
        ),
        (
            "right",
            "int base_right(void); int right(void) { return base_right(); }",
        ),
    ] {
        std::fs::write(workspace.path().join(format!("{name}.c")), source).unwrap();
        let output = Command::new("cc")
            .current_dir(workspace.path())
            .args(["-c", &format!("{name}.c"), "-o", &format!("{name}.o")])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (name, objects) in [
        ("base", vec!["base_left.o", "base_right.o"]),
        ("left", vec!["left.o"]),
        ("right", vec!["right.o"]),
    ] {
        let output = Command::new("ar")
            .current_dir(workspace.path())
            .args(["rcs", &format!("lib{name}.a")])
            .args(objects)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(
        workspace.path().join("main.c"),
        "int left(void); int right(void); int main(void) { return left() + right() != 10; }",
    )
    .unwrap();
    let output = Command::new("cc")
        .current_dir(workspace.path())
        .args(["main.c", "-L."])
        .args(
            providers[3]["transitive_linkopts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        )
        .args(["-o", "app"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Command::new(workspace.path().join("app"))
        .status()
        .unwrap()
        .success());
}
