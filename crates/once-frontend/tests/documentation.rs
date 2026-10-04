use std::path::Path;

use once_frontend::built_in_target_kind_schemas_result;

#[test]
fn every_builtin_target_kind_has_a_public_reference_page() {
    let reference =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/priv/docs/reference/prelude");
    let schemas = built_in_target_kind_schemas_result().unwrap();
    let missing = schemas
        .iter()
        .filter(|schema| !reference.join(format!("{}.md", schema.kind)).is_file())
        .map(|schema| schema.kind.as_str())
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "target kinds without documentation: {missing:?}"
    );
}
