//! Per-thread evaluation state and the core `eval_with` driver.
//!
//! `eval_with` installs an [`EvalState`] in the thread-local before
//! invoking the Starlark evaluator and takes it back when evaluation
//! finishes. Starlark evaluation is synchronous, so nothing else on
//! this thread can observe the slot mid-load. Globals (defined in
//! [`crate::globals`]) reach into the slot via [`with_state`] to record
//! targets and resolve `glob` patterns.

use std::cell::RefCell;
use std::path::PathBuf;

use starlark::environment::{Globals, GlobalsBuilder, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

use crate::error::{Error, Result};
use crate::globals::fabrik_globals;
use crate::prelude::{PRELUDE_NAME, PRELUDE_SOURCE};
use crate::target::Target;

pub(crate) struct EvalState {
    pub workspace_root: PathBuf,
    pub package: String,
    pub targets: Vec<Target>,
}

thread_local! {
    static STATE: RefCell<Option<EvalState>> = const { RefCell::new(None) };
}

pub(crate) fn with_state<R>(f: impl FnOnce(&mut EvalState) -> R) -> R {
    STATE.with(|c| {
        let mut borrow = c.borrow_mut();
        let state = borrow
            .as_mut()
            .expect("evaluation state installed by eval_with");
        f(state)
    })
}

/// Evaluate `src` under `name`, returning the targets it declares. The
/// evaluator can read `workspace_root`/`package` via [`with_state`] so
/// `glob` resolves against the right directory and recorded targets
/// inherit the right package label.
pub(crate) fn eval_with(
    name: &str,
    src: &str,
    workspace_root: PathBuf,
    package: String,
) -> Result<Vec<Target>> {
    let user_ast =
        AstModule::parse(name, src.to_owned(), &Dialect::Standard).map_err(|e| Error::Parse {
            path: name.to_owned(),
            message: format!("{e:#}"),
        })?;
    // The bundled prelude defines the typed wrappers (rust_binary,
    // rust_library, rust_test) on top of the `target` primitive. It is
    // evaluated into the same Module as the user file so its top-level
    // `def` bindings are visible to user code without an explicit
    // `load(...)`. Parse failure here is a packaging bug, not a user
    // error.
    let prelude_ast = AstModule::parse(PRELUDE_NAME, PRELUDE_SOURCE.to_owned(), &Dialect::Standard)
        .expect("bundled prelude parses");
    let globals: Globals = GlobalsBuilder::new().with(fabrik_globals).build();
    let module = Module::new();

    STATE.with(|c| {
        *c.borrow_mut() = Some(EvalState {
            workspace_root,
            package,
            targets: Vec::new(),
        });
    });
    let eval_result = (|| -> std::result::Result<(), starlark::Error> {
        let mut eval = Evaluator::new(&module);
        eval.eval_module(prelude_ast, &globals)?;
        eval.eval_module(user_ast, &globals)?;
        Ok(())
    })();
    let collected = STATE
        .with(|c| c.borrow_mut().take())
        .map(|s| s.targets)
        .unwrap_or_default();

    eval_result.map_err(|e| Error::Eval {
        path: name.to_owned(),
        message: format!("{e:#}"),
    })?;
    Ok(collected)
}

/// Evaluate `fabrik.star` source as if it lived at the workspace root.
/// `glob` is unavailable in this mode (the package directory is unset);
/// intended for tests and ad-hoc evaluation.
pub fn load_str(name: &str, src: &str) -> Result<Vec<Target>> {
    eval_with(name, src, PathBuf::from("."), String::new())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn declares_a_rust_binary() {
        let src = r#"
rust_binary(
    name = "hello",
    srcs = ["src/main.rs"],
)
"#;
        let targets = load_str("fabrik.star", src).unwrap();
        assert_eq!(
            targets,
            vec![Target {
                package: String::new(),
                kind: "rust_binary".into(),
                name: "hello".into(),
                srcs: vec!["src/main.rs".into()],
                deps: vec![],
                attrs: BTreeMap::new(),
            }]
        );
    }

    #[test]
    fn preserves_source_order_across_kinds() {
        let src = r#"
rust_library(name = "core", srcs = ["lib.rs"])
rust_binary(name = "cli", srcs = ["main.rs"], deps = [":core"])
rust_test(name = "core_test", srcs = ["tests/core.rs"], deps = [":core"])
"#;
        let kinds: Vec<_> = load_str("fabrik.star", src)
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, t.name))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("rust_library".into(), "core".into()),
                ("rust_binary".into(), "cli".into()),
                ("rust_test".into(), "core_test".into()),
            ]
        );
    }

    #[test]
    fn missing_name_is_an_evaluation_error() {
        let err = load_str("fabrik.star", "rust_binary(srcs = [\"a.rs\"])").unwrap_err();
        assert!(matches!(err, Error::Eval { .. }), "got {err:?}");
    }

    #[test]
    fn syntax_error_is_a_parse_error() {
        let err = load_str("fabrik.star", "rust_binary(name = ").unwrap_err();
        assert!(matches!(err, Error::Parse { .. }), "got {err:?}");
    }

    #[test]
    fn empty_file_yields_no_targets() {
        let targets = load_str("fabrik.star", "").unwrap();
        assert!(targets.is_empty());
    }

    #[test]
    fn non_string_in_srcs_is_an_evaluation_error() {
        let err = load_str("fabrik.star", "rust_binary(name = \"x\", srcs = [1, 2])").unwrap_err();
        assert!(matches!(err, Error::Eval { .. }), "got {err:?}");
    }

    #[test]
    fn target_primitive_is_directly_callable() {
        // The prelude's typed wrappers are sugar over `target`. User
        // code (and future plugin authors) can call the primitive
        // directly with arbitrary kinds.
        let targets = load_str(
            "fabrik.star",
            "target(kind = \"custom\", name = \"x\", srcs = [\"a.rs\"], deps = [])",
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "custom");
        assert_eq!(targets[0].name, "x");
    }

    #[test]
    fn cargo_binary_records_cargo_attrs() {
        let targets = load_str(
            "fabrik.star",
            r#"cargo_binary(
    name = "fabrik",
    cargo_package = "fabrik-cli",
    bin = "fabrik",
    srcs = ["Cargo.toml"],
)"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "cargo_binary");
        assert_eq!(targets[0].attrs["cargo_package"], "fabrik-cli");
        assert_eq!(targets[0].attrs["bin"], "fabrik");
    }
}
