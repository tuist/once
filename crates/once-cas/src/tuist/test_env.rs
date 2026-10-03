//! Test-only lock shared by modules whose tests mutate process environment
//! variables. Environment variables are process-global, so tests that read or
//! write them must serialize even when they live in different modules.

use std::sync::Mutex;

pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());
