use super::*;
use std::os::unix::fs::PermissionsExt;

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
    for (existing_checkout, reject_shallow, reject_full, missing_revision) in [
        (false, true, true, false),
        (true, false, true, false),
        (true, true, false, false),
        (true, true, false, true),
    ] {
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
        let wrapper = workspace.path().join("git-fixture");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\ncase \" $* \" in\n*' fetch --depth '*) if {reject_shallow}; then echo 'Server does not support shallow clients' >&2; exit 1; fi;;\n*' fetch origin '*) if {reject_full}; then echo 'unexpected full fetch' >&2; exit 1; fi;;\nesac\nexec git \"$@\"\n"
            ),
        ).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let source = format!(
            r#"{}
def host_which(name):
    return {wrapper:?} if name == "git" else "/usr/bin/" + name

def _xcode_swift_package_info(ctx, package_dir, identity, cache = None):
    return package_dir

result = repr(_xcode_remote_swift_package_info({{}}, "pkg", {{"state": {{"revision": {revision:?}}}}}, url = {remote:?}))
"#,
            xcode_prelude_source(),
            wrapper = wrapper.to_str().unwrap(),
            remote = remote.to_str().unwrap(),
        );
        let store = store_for(workspace.path(), "");
        let (_, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
        if missing_revision {
            assert!(result.unwrap_err().contains("exited with"));
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
