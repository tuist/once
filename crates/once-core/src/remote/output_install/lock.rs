use std::path::Path;
use std::time::{Duration, Instant};

use crate::remote::path::Directory;
use crate::{Error, Result};

pub(super) fn with_output_lock<T>(
    workspace: &Directory,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let path = Path::new(".once/locks/output-restore.lock");
    let error = |source| Error::RestoreOutput {
        path: path.display().to_string(),
        source,
    };
    let file = workspace.open_or_create_file(path).map_err(error)?;
    let mut lock = fd_lock::RwLock::new(file);
    let started = Instant::now();
    loop {
        match lock.try_write() {
            Ok(_guard) => return operation(),
            Err(source)
                if source.kind() == std::io::ErrorKind::WouldBlock
                    && started.elapsed() < Duration::from_secs(30) =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(source) => return Err(error(source)),
        }
    }
}
