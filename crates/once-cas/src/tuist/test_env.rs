//! Isolated process-environment changes for tests.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub(crate) fn acquire(names: &[&'static str]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in names {
            std::env::remove_var(name);
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Restoration must finish before the lock field is dropped.
        for (name, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_holds_the_lock_until_environment_is_restored() {
        let original = std::env::var_os("ONCE_ENV_GUARD_TEST");
        let guard = EnvGuard::acquire(&["ONCE_ENV_GUARD_TEST"]);
        std::env::set_var("ONCE_ENV_GUARD_TEST", "changed");
        assert!(ENV_LOCK.try_lock().is_err());
        drop(guard);
        let _lock = ENV_LOCK.lock().unwrap();
        assert_eq!(std::env::var_os("ONCE_ENV_GUARD_TEST"), original);
    }
}
