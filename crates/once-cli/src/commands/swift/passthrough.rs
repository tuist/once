use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;

pub(super) fn run(argv: &[String]) -> Result<ExitCode> {
    crate::commands::compatibility::run_passthrough(argv, "swift", "ONCE_SWIFT_PATH")
}

pub(super) fn system_swift() -> Result<PathBuf> {
    crate::commands::compatibility::system_executable("swift", "ONCE_SWIFT_PATH")
}
