use std::path::Path;

fn main() {
    // Cargo documents every one of these for a build script, and real
    // packages panic when one is missing.
    for name in [
        "CARGO_MANIFEST_DIR", "CARGO_PKG_NAME", "CARGO_PKG_VERSION", "DEBUG", "HOST", "NUM_JOBS",
        "OPT_LEVEL", "OUT_DIR", "PROFILE", "RUSTC", "RUSTDOC", "TARGET",
    ] {
        std::env::var(name).unwrap_or_else(|_| panic!("missing build script variable {name}"));
    }
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(
        Path::new(&out_dir).join("registry.rs"),
        format!("pub const REGISTRY: &str = \"{}\";\n", mise_registry::registry()),
    )
    .expect("failed to write the generated registry");
    println!("cargo:rerun-if-changed=build.rs");
}
