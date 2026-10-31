use super::*;

fn library_hmap(umbrella: bool) -> (TempDir, Vec<u8>, String, Vec<String>) {
    let workspace = TempDir::new().unwrap();
    for (path, contents) in [
        ("Pkg/Sources/Value.swift", "public func value() {}\n"),
        ("Pkg/Sources/Use.c", "int use(void) { return 0; }\n"),
        ("Pkg/Sources/Private.h", "#define PRIVATE_VALUE 2\n"),
        (
            "Pkg/Sources/Value.h",
            "#error private must not shadow public\n",
        ),
        ("Pkg/include/Value.h", "#define PUBLIC_VALUE 1\n"),
        ("Pkg/include/Base.h", "#include \"Value.h\"\n"),
    ] {
        let path = workspace.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    let source = format!(
        r#"{}
def host_arch():
    return "arm64"
def host_command(argv, env = None, merge_stderr = None):
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host command: " + str(argv))
ctx = {{
    "label": {{"package": "Pkg", "name": "Base", "id": "Pkg/Base"}},
    "attr": {{
        "platform": "ios", "sdk_variant": "simulator",
        "xcode_developer_dir": "/opt/Xcode/Developer", "enable_modules": True,
        "exported_headers": {}, "private_header_dirs": ["Sources", "."],
    }},
    "deps": [], "srcs": ["Sources/*.swift", "Sources/*.c"],
    "build_dir": ".once/out/Pkg/Base", "capability": "build",
}}
result = repr(_apple_library_impl(ctx))
"#,
        apple_prelude_source(),
        if umbrella {
            r#"["include/Base.h", "include/Value.h"]"#
        } else {
            r#"["include/Value.h"]"#
        },
    );
    let (store, result) = with_active_store(store_for(workspace.path(), "Pkg"), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    let action = store
        .actions
        .iter()
        .find(|action| {
            action
                .outputs
                .iter()
                .any(|path| path.ends_with("Base.hmap"))
        })
        .unwrap();
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &action.operation else {
        panic!("header map must be a write action");
    };
    let generated_header = if umbrella {
        ".once/out/Pkg/Base.framework/Headers/Base-Swift.h"
    } else {
        ".once/out/Pkg/Base/Headers/Base/Base-Swift.h"
    };
    let compiler = store
        .actions
        .iter()
        .find(|action| {
            action
                .argv
                .last()
                .is_some_and(|arg| arg == "Pkg/Sources/Use.c")
        })
        .unwrap();
    let search_flags = compiler
        .argv
        .windows(2)
        .filter(|args| args[0] == "-I")
        .flat_map(<[std::string::String]>::to_vec)
        .collect();
    if !umbrella {
        let swift = action_by_identifier(&store, "swift_module_compile_Base");
        assert!(swift.argv.windows(8).any(|args| {
            args == [
                "-Xcc",
                "-I",
                "-Xcc",
                ".once/out/Pkg/Base.hmap",
                "-Xcc",
                "-I",
                "-Xcc",
                ".",
            ]
        }));
    }
    (
        workspace,
        bytes.clone(),
        generated_header.to_string(),
        search_flags,
    )
}

fn hmap_entries(bytes: &[u8]) -> BTreeMap<String, String> {
    let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    let strings = word(8);
    let string = |offset| {
        let tail = &bytes[strings + offset..];
        std::str::from_utf8(&tail[..tail.iter().position(|byte| *byte == 0).unwrap()])
            .unwrap()
            .to_string()
    };
    let mut entries = BTreeMap::new();
    for bucket in 0..word(16) {
        let offset = 24 + bucket * 12;
        if word(offset) != 0 {
            entries.insert(
                string(word(offset)),
                string(word(offset + 4)) + &string(word(offset + 8)),
            );
        }
    }
    entries
}

#[test]
fn hmap_values_are_execution_root_relative_for_public_private_and_swift_headers() {
    for umbrella in [false, true] {
        let (_workspace, bytes, generated_header, _) = library_hmap(umbrella);
        let entries = hmap_entries(&bytes);
        for key in ["Value.h", "Base/Value.h"] {
            assert_eq!(entries[key], "Pkg/include/Value.h");
        }
        for key in ["Private.h", "Base/Private.h"] {
            assert_eq!(entries[key], "Pkg/Sources/Private.h");
        }
        assert_eq!(entries["Base/Base-Swift.h"], generated_header);
        assert!(entries.values().all(|path| Path::new(path).is_relative()));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn clang_reads_generated_hmaps_locally_and_after_execution_root_relocation() {
    for umbrella in [false, true] {
        let (workspace, bytes, generated_header, search_flags) = library_hmap(umbrella);
        let worker = TempDir::new().unwrap();
        for root in [workspace.path(), worker.path()] {
            for (path, contents) in [
                ("Pkg/include/Value.h", "#define PUBLIC_VALUE 1\n"),
                ("Pkg/Sources/Private.h", "#define PRIVATE_VALUE 2\n"),
                (generated_header.as_str(), "#define SWIFT_VALUE 3\n"),
                (
                    "Use.c",
                    "#include <Base/Value.h>\n#include <Base/Private.h>\n#include <Base/Base-Swift.h>\n_Static_assert(PUBLIC_VALUE + PRIVATE_VALUE + SWIFT_VALUE == 6, \"headers\");\n",
                ),
            ] {
                let path = root.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, contents).unwrap();
            }
            let map = root.join(".once/out/Pkg/Base.hmap");
            std::fs::create_dir_all(map.parent().unwrap()).unwrap();
            std::fs::write(map, &bytes).unwrap();
        }
        let compile = |root: &Path| {
            let result = Command::new("/usr/bin/clang")
                .current_dir(root)
                .arg("-fsyntax-only")
                .args(&search_flags)
                .arg("Use.c")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "umbrella={umbrella}, root={root:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        compile(workspace.path());
        workspace.close().unwrap();
        compile(worker.path());
    }
}

#[test]
fn xcframework_import_defaults_to_execution_arch_and_preserves_explicit_override() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
def host_arch():
    return "arm64"
execution = {{}}
def execution_context():
    return execution
def host_file_exists(path):
    return True
def host_which(name):
    return name
def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "plutil":
        return _json_encode({{"AvailableLibraries": [
            {{"LibraryIdentifier": "ios-" + arch + suffix, "LibraryPath": "libValue.a", "SupportedPlatform": "ios", "SupportedPlatformVariant": variant, "SupportedArchitectures": [arch]}}
            for arch in ["arm64", "x86_64"]
            for variant, suffix in [("simulator", "-simulator"), ("", "")]
        ]}})
    if argv[0] == "find":
        return argv[2] + "/libValue.a\n"
    fail("unexpected host command: " + str(argv))
ctx = {{
    "label": {{"package": "", "name": "Imported", "id": "Imported"}},
    "attr": {{"bundle": "Value.xcframework", "platform": "ios", "sdk_variant": "simulator"}},
    "deps": [],
}}
local = _apple_xcframework_import_impl(ctx)["transitive_archives"]
worker = {{"schema_version": 1, "environment": "image", "os": "linux", "arch": "x86_64", "bin": "/usr/bin", "developer_dir": "/sdk/Developer", "tools": "/tools", "sdks": {{}}}}
execution.update({{"provider": "workers", "executor": "docker", "environment": "image", "properties": {{"apple": worker}}}})
remote = _apple_xcframework_import_impl(ctx)["transitive_archives"]
ctx["attr"]["arch"] = "arm64"
explicit = _apple_xcframework_import_impl(ctx)["transitive_archives"]
ctx["attr"].pop("arch")
ctx["attr"]["sdk_variant"] = "device"
device = _apple_xcframework_import_impl(ctx)["transitive_archives"]
result = repr([local, remote, explicit, device])
"#,
        apple_prelude_source(),
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["Value.xcframework/ios-arm64-simulator/libValue.a"], ["Value.xcframework/ios-x86_64-simulator/libValue.a"], ["Value.xcframework/ios-arm64-simulator/libValue.a"], ["Value.xcframework/ios-arm64/libValue.a"]]"#
    );
}
