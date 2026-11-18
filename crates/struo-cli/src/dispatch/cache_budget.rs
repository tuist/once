use crate::cli::Cmd;
use anyhow::{Context, Result};
use struo_core::ResourceLimits;

pub(super) fn configure(
    limits: &mut ResourceLimits,
    command: &Cmd,
    remote_selected: bool,
) -> Result<u64> {
    let enabled = reserve_headroom(command, remote_selected);
    let (actions, cache) = split(limits.memory_bytes, enabled)?;
    if enabled {
        crate::cache_provider::configure_transfer_budget(struo_cas::TransferBudget::new(
            cache,
            struo_cas::DEFAULT_TRANSFER_STAGING_BYTES,
        ));
        *limits = limits.clone().with_memory_bytes(actions);
    }
    Ok(cache)
}

fn reserve_headroom(command: &Cmd, remote_selected: bool) -> bool {
    remote_selected || matches!(command, Cmd::Mcp { .. } | Cmd::Runtime { .. })
}

fn split(total: u64, enabled: bool) -> Result<(u64, u64)> {
    if !enabled {
        return Ok((total, 0));
    }
    if total == 0 {
        return Ok((0, struo_cas::DEFAULT_TRANSFER_MEMORY_BYTES));
    }
    let minimum = struo_cas::MIN_TRANSFER_MEMORY_BYTES;
    if total <= minimum {
        return Err(struo_core::Error::ResourceRequirementUnsatisfied {
            resource: "command memory for remote cache transfers and action admission".into(),
            requested: minimum + 1,
            capacity: total,
        })
        .with_context(|| format!("increase --memory-limit above {minimum} bytes to leave room for cache transfers and actions"));
    }
    let cache = (total / 8).clamp(minimum, struo_cas::DEFAULT_TRANSFER_MEMORY_BYTES);
    Ok((total - cache, cache))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lived_local_sessions_reserve_headroom_for_provider_changes() {
        assert!(reserve_headroom(
            &Cmd::Mcp {
                workspace: None,
                allow_run: false
            },
            false
        ));
        assert!(reserve_headroom(&Cmd::Runtime { cmd: None }, false));
    }

    #[test]
    fn small_child_budgets_still_fit_a_complete_stream_frame() {
        for total in [250 * 1024 * 1024, 256 * 1024 * 1024, 300 * 1024 * 1024] {
            let (actions, cache) = split(total, true).unwrap();
            assert!(cache >= struo_cas::MIN_TRANSFER_MEMORY_BYTES);
            assert!(actions > 0);
            assert_eq!(actions + cache, total);
        }
    }

    #[test]
    fn insufficient_total_fails_instead_of_disabling_action_admission() {
        for total in [1, 16 * 1024 * 1024, struo_cas::MIN_TRANSFER_MEMORY_BYTES] {
            let error = split(total, true).unwrap_err();
            assert!(error.to_string().contains("--memory-limit"));
            assert!(matches!(
                error.downcast_ref::<struo_core::Error>(),
                Some(struo_core::Error::ResourceRequirementUnsatisfied { .. })
            ));
        }
        let (actions, cache) = split(struo_cas::MIN_TRANSFER_MEMORY_BYTES + 1, true).unwrap();
        assert_eq!(actions, 1);
        assert_eq!(cache, struo_cas::MIN_TRANSFER_MEMORY_BYTES);
    }

    #[test]
    fn local_only_work_keeps_its_budget_and_unbounded_mode_stays_explicit() {
        assert_eq!(
            split(16 * 1024 * 1024, false).unwrap(),
            (16 * 1024 * 1024, 0)
        );
        assert_eq!(
            split(0, true).unwrap(),
            (0, struo_cas::DEFAULT_TRANSFER_MEMORY_BYTES)
        );
        let (actions, cache) = split(8 * 1024 * 1024 * 1024, true).unwrap();
        assert_eq!(cache, struo_cas::DEFAULT_TRANSFER_MEMORY_BYTES);
        assert_eq!(actions + cache, 8 * 1024 * 1024 * 1024);
    }
}
