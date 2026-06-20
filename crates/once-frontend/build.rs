use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"),
    );
    emit_rerun_if_changed(&manifest_dir.join("prelude"))
}

fn emit_rerun_if_changed(path: &Path) -> io::Result<()> {
    println!("cargo:rerun-if-changed={}", path.display());

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            emit_rerun_if_changed(&entry?.path())?;
        }
    }

    Ok(())
}
