// Injects the release version into the binary so `once` reports the
// same string the release workflow published, instead of the
// workspace `Cargo.toml`'s placeholder `0.0.0`.
//
// Contract:
//   * If `ONCE_RELEASE_VERSION` is set at build time, that's the
//     version the binary reports (the release workflow sets it to
//     the tag detected by `mise run release:detect`).
//   * Otherwise the binary reports the workspace crate version, so
//     local `cargo build` / `once cargo build` keeps working with no
//     env setup.
//
// The `rerun-if-env-changed` directive forces a rebuild whenever the
// release version changes, so a cached artifact from a previous build
// never keeps stamping the wrong string.
fn main() {
    println!("cargo:rerun-if-env-changed=ONCE_RELEASE_VERSION");
    let version = std::env::var("ONCE_RELEASE_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=ONCE_VERSION={version}");
}
