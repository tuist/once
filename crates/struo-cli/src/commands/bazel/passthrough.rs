use std::process::ExitCode;

use anyhow::Result;

pub(super) fn run(argv: &[String]) -> Result<ExitCode> {
    crate::commands::compatibility::run_passthrough(argv, "bazel", "STRUO_BAZEL_PATH")
}
