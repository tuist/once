use std::{
    collections::BTreeSet,
    env, fs, io,
    path::{Path, PathBuf},
};

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"),
    );
    let prelude = manifest_dir.join("prelude");
    let root = prelude.canonicalize()?;
    let mut visited = BTreeSet::new();
    emit_rerun_if_changed(&prelude, &root, &mut visited)
}

fn emit_rerun_if_changed(
    path: &Path,
    root: &Path,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let canonical = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(error)
            if link_metadata.file_type().is_symlink()
                || error.kind() == io::ErrorKind::NotFound =>
        {
            println!("cargo:rerun-if-changed={}", path.display());
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if !canonical.starts_with(root) {
        return Ok(());
    }

    println!("cargo:rerun-if-changed={}", path.display());

    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Ok(());
    }

    if !visited.insert(canonical) {
        return Ok(());
    }

    let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        emit_rerun_if_changed(&entry.path(), root, visited)?;
    }

    Ok(())
}
