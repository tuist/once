//! Derive a stable musical seed from a `Cmd` for the sound sink.
//!
//! Kept in its own file so the ecosystem-flavored strings that identify
//! compatibility wrappers do not leak into `dispatch.rs`, which the
//! architecture test in `tests/test_architecture.rs` requires to remain
//! ecosystem-neutral. Same input command always produces the same seed, so
//! every invocation of a given command sounds like the same piece of music.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::cli::Cmd;

pub(crate) fn for_command(command: &Cmd) -> u64 {
    let mut hasher = DefaultHasher::new();
    match command {
        Cmd::Build { target, .. } => {
            "build".hash(&mut hasher);
            target.hash(&mut hasher);
        }
        Cmd::Lint { target, .. } => {
            "lint".hash(&mut hasher);
            target.hash(&mut hasher);
        }
        Cmd::Run {
            target, arguments, ..
        } => {
            "run".hash(&mut hasher);
            target.hash(&mut hasher);
            arguments.hash(&mut hasher);
        }
        Cmd::Test {
            target,
            changed_paths,
            test_unit,
            all,
            ..
        } => {
            "test".hash(&mut hasher);
            target.hash(&mut hasher);
            changed_paths.hash(&mut hasher);
            test_unit.hash(&mut hasher);
            all.hash(&mut hasher);
        }
        Cmd::Exec { argv, .. } => {
            "exec".hash(&mut hasher);
            argv.hash(&mut hasher);
        }
        Cmd::Compatibility { argv } => {
            "xcodebuild".hash(&mut hasher);
            argv.hash(&mut hasher);
        }
        Cmd::PackageCompatibility { argv } => {
            "swift".hash(&mut hasher);
            argv.hash(&mut hasher);
        }
        Cmd::BazelCompatibility { argv } => {
            "bazel".hash(&mut hasher);
            argv.hash(&mut hasher);
        }
        Cmd::CrateCompatibility { argv } => {
            "cargo".hash(&mut hasher);
            argv.hash(&mut hasher);
        }
        _ => {}
    }
    hasher.finish()
}
