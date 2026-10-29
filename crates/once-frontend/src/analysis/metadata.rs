use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::store::HOST_COMMAND_OUTPUT_LIMIT;

pub(super) fn environment_digest() -> String {
    let environment = std::env::vars_os().collect::<BTreeMap<_, _>>();
    let mut digest = Sha256::new();
    for (name, value) in environment {
        for part in [name, value] {
            let bytes = part.as_encoded_bytes();
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
    }
    super::globals::hex_lower(&digest.finalize())
}

#[derive(Deserialize, Serialize)]
struct Stored {
    key: String,
    output: String,
}

pub(super) fn command(
    workspace: &Path,
    slot: &str,
    key: &str,
    run: impl FnOnce() -> Result<String>,
) -> Result<String> {
    let directory = workspace.join(".once/metadata-v1");
    let path = directory.join(slot);
    if let Ok(file) = std::fs::File::open(&path) {
        let mut bytes = Vec::new();
        file.take(HOST_COMMAND_OUTPUT_LIMIT * 2)
            .read_to_end(&mut bytes)?;
        if let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) {
            if stored.key == key && stored.output.len() as u64 <= HOST_COMMAND_OUTPUT_LIMIT {
                return Ok(stored.output);
            }
        }
    }
    let output = run()?;
    std::fs::create_dir_all(&directory).context("creating metadata cache")?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(
        &mut temporary,
        &Stored {
            key: key.into(),
            output: output.clone(),
        },
    )?;
    temporary.flush()?;
    temporary
        .persist(path)
        .context("publishing metadata cache")?;
    Ok(output)
}
