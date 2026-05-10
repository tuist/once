//! Apple platform support.
//!
//! This crate is the first official plugin-style slice for Apple
//! targets. It models an iOS simulator app as direct tool actions:
//! `swiftc` compiles sources into an `.app` bundle, `codesign` signs it
//! ad hoc for the simulator, and `simctl` installs and launches it for
//! task execution.

mod artifact;
mod compile;
mod plan;
mod swift;

pub use artifact::app_bundle_path;
pub use compile::{compile_ios_app, launch_ios_app, AppleAction, AppleError};
pub use plan::{build_plan, supports_kind, PlanBuildError};
pub use swift::{compile_swift_target, SwiftError};
