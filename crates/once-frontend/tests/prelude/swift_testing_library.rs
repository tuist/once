use super::{apple_prelude_source, eval_prelude_source_to_repr};

fn evaluate(body: &str) -> String {
    eval_prelude_source_to_repr(format!("{}\n{body}", apple_prelude_source())).unwrap()
}

/// Xcode publishes Swift Testing as a platform framework, and a Swift
/// toolchain installed beside Xcode ships its own copy next to the
/// compiler. Only the copy that belongs to the compiler matches the macro
/// plugin that same compiler loads, so it has to win when it is present.
#[test]
fn prefers_the_testing_library_that_ships_with_the_compiler() {
    let result = evaluate(
        r#"
def host_file_exists(path):
    return path.startswith("/Toolchains/snapshot.xctoolchain/")

result = repr([
    _swift_testing_library_dir("/Toolchains/snapshot.xctoolchain/usr/bin/swiftc", "macosx"),
    _swift_testing_library_dir("/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc", "iphonesimulator"),
])
"#,
    );
    assert_eq!(
        result,
        r#"["/Toolchains/snapshot.xctoolchain/usr/lib/swift/macosx/testing", ""]"#
    );
}

/// Compiling against the compiler's own copy is a module search path, and
/// linking it is a dynamic library reached through an rpath rather than a
/// framework the platform already exposes. Without a toolchain copy both
/// fall back to the platform framework.
#[test]
fn compiles_and_links_against_whichever_testing_library_applies() {
    let result = evaluate(
        r#"
result = repr([
    _apple_swift_testing_compile_flags("/toolchain/testing", "/platform/Frameworks", "/plugin.dylib"),
    _apple_swift_testing_link_flags("/toolchain/testing"),
    _apple_swift_testing_compile_flags("", "/platform/Frameworks", "/plugin.dylib"),
    _apple_swift_testing_link_flags(""),
])
"#,
    );
    assert_eq!(
        result,
        r#"[["-I", "/toolchain/testing", "-load-plugin-library", "/plugin.dylib"], ["-L", "/toolchain/testing", "-lTesting", "-Xlinker", "-rpath", "-Xlinker", "/toolchain/testing"], ["-F", "/platform/Frameworks", "-framework", "Testing", "-load-plugin-library", "/plugin.dylib"], ["-framework", "Testing"]]"#
    );
}
