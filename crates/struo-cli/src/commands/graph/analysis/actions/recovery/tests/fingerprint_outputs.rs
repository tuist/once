use super::*;

mod boundaries;
mod cached_consumer;
mod cross_target;

fn graph(produce: &str, clean: bool, consume: &str) -> AnalysisResult {
    let mut actions = vec![action(serde_json::json!({
        "identifier": "produce", "argv": ["/bin/sh", "-c", produce],
        "outputs": [PRODUCED], "create_dirs": [".struo/out"], "cacheable": false,
    }))];
    if clean {
        actions.push(action(serde_json::json!({
            "identifier": "clean", "argv": ["/bin/sh", "-c", "printf cleaned > .struo/out/cleaned.txt"],
            "outputs": [".struo/out/cleaned.txt"], "clean_paths": [PRODUCED], "cacheable": false,
        })));
    }
    actions.push(action(serde_json::json!({
        "identifier": "consume", "argv": ["/bin/sh", "-c", consume],
        "inputs": [PRODUCED], "outputs": [".struo/out/consumed.txt"], "cacheable": false,
    })));
    AnalysisResult {
        actions,
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    }
}

#[tokio::test]
async fn removed_uncached_symlink_fails_instead_of_becoming_manifest_text() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let analysis = graph(
        &format!("printf original > .struo/out/source.txt; ln -s source.txt {PRODUCED}"),
        true,
        &format!("cat {PRODUCED} > .struo/out/consumed.txt"),
    );
    let error = workspace
        .build_with(analysis, &producers, None)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("fingerprint-only output"),
        "{error:#}"
    );
    assert!(!workspace.path().join(PRODUCED).exists());
    assert!(!workspace.path().join(".struo/out/consumed.txt").exists());
}

#[tokio::test]
async fn removed_uncached_directory_fails_before_installing_manifest_text() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let analysis = graph(
        &format!("mkdir {PRODUCED}; printf original > {PRODUCED}/child"),
        true,
        &format!("cat {PRODUCED}/child > .struo/out/consumed.txt"),
    );
    let error = workspace
        .build_with(analysis, &producers, None)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("fingerprint-only output"),
        "{error:#}"
    );
    assert!(!workspace.path().join(PRODUCED).exists());
    assert!(!workspace.path().join(".struo/out/consumed.txt").exists());
}

#[tokio::test]
async fn live_uncached_directory_and_symlink_remain_usable() {
    for (produce, consume) in [
        (
            format!("printf original > .struo/out/source.txt; ln -s source.txt {PRODUCED}"),
            format!("cat {PRODUCED} > .struo/out/consumed.txt"),
        ),
        (
            format!("mkdir {PRODUCED}; printf original > {PRODUCED}/child"),
            format!("cat {PRODUCED}/child > .struo/out/consumed.txt"),
        ),
    ] {
        let workspace = Workspace::new();
        workspace
            .build_with(
                graph(&produce, false, &consume),
                &Producers::default(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join(".struo/out/consumed.txt")).unwrap(),
            b"original"
        );
    }
}
