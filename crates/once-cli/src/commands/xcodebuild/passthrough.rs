use std::process::ExitCode;

use anyhow::Result;

pub(super) fn run(argv: &[String]) -> Result<ExitCode> {
    crate::commands::compatibility::run_passthrough(argv, "xcodebuild", "ONCE_XCODEBUILD_PATH")
}
