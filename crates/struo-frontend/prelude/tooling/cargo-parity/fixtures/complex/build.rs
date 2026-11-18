use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let value = generator::generate(include_str!("schema.txt"));
    fs::write(
        out.join("generated.rs"),
        format!("pub const GENERATED: u32 = {value};\n"),
    )
    .unwrap();
    let config = [
        "HOST",
        "TARGET",
        "PROFILE",
        "OPT_LEVEL",
        "DEBUG",
        "CARGO_FEATURE_EXTRA",
    ]
    .map(|key| format!("{key}={}\n", env::var(key).unwrap_or_default()))
    .concat();
    fs::write(out.join("configuration.txt"), config).unwrap();
    println!("cargo:rustc-cfg=generated_input");
    println!("cargo:rustc-check-cfg=cfg(generated_input)");
    println!("cargo:rustc-env=E7_MARKER=generated");
    println!("cargo:rerun-if-changed=schema.txt");
}
