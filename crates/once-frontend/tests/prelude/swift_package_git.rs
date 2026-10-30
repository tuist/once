use super::*;

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn remote_swift_package_checks_out_pinned_commits_with_real_git() {
    for (existing_checkout, missing_revision) in [(false, false), (true, false), (true, true)] {
        let workspace = TempDir::new().unwrap();
        let remote = workspace.path().join("remote");
        let checkout = workspace.path().join(".once/xcode-packages/pkg");
        std::fs::create_dir(&remote).unwrap();
        git(&remote, &["init"]);
        std::fs::write(remote.join("Package.swift"), "// first revision\n").unwrap();
        git(&remote, &["add", "."]);
        git(
            &remote,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "-m",
                "first",
            ],
        );
        if existing_checkout {
            std::fs::create_dir_all(checkout.parent().unwrap()).unwrap();
            git(
                workspace.path(),
                &[
                    "clone",
                    remote.to_str().unwrap(),
                    checkout.to_str().unwrap(),
                ],
            );
        }
        std::fs::write(remote.join("Package.swift"), "// pinned revision\n").unwrap();
        git(&remote, &["add", "."]);
        git(
            &remote,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "-m",
                "pinned",
            ],
        );
        let revision = if missing_revision {
            "f".repeat(40)
        } else {
            git(&remote, &["rev-parse", "HEAD"])
        };
        let source = format!(
            r#"{}
def host_env(name):
    return {cache:?}

def _xcode_swift_package_info(ctx, package_dir, identity, cache = None):
    return package_dir

result = repr(_xcode_remote_swift_package_info({{}}, "pkg", {{"state": {{"revision": {revision:?}}}}}, url = {remote:?}))
"#,
            xcode_prelude_source(),
            cache = workspace.path().join("cache").to_str().unwrap(),
            remote = remote.to_str().unwrap(),
        );
        let store = store_for(workspace.path(), "");
        let (_, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
        if missing_revision {
            assert!(result
                .unwrap_err()
                .contains("repository acquisition failed"));
        } else {
            assert_eq!(result.unwrap(), r#"".once/xcode-packages/pkg""#);
            assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), revision);
            assert_eq!(
                std::fs::read_to_string(checkout.join("Package.swift")).unwrap(),
                "// pinned revision\n"
            );
        }
    }
}
