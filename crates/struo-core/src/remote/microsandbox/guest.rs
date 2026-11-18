use std::future::Future;
use std::time::Duration;

use crate::resources::DEFAULT_ACTION_MEMORY_BYTES;
use crate::{Error, ResourceRequest, Result};

const MIB: u64 = 1024 * 1024;

/// How long a guest gets to shut down after the stop request.
pub(super) const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a killed guest gets to exit before cleanup gives up on it.
pub(super) const KILL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long removing a stopped guest's persisted state may take.
pub(super) const REMOVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Virtual machine size, derived from what the host actually reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GuestSize {
    pub cpus: u8,
    pub memory_mib: u32,
}

impl GuestSize {
    /// Size the guest from the admitted host reservation so the VM never gets
    /// more than admission charged for it: admitted memory is rounded down to
    /// whole MiB, and a reservation below one MiB cannot host a guest. Callers
    /// that run without admission, or against a pool without a memory axis,
    /// fall back to the declared estimate, then to the generic default action
    /// estimate, never to the VM runtime's own unaccounted default.
    pub(super) fn for_request(
        admitted: Option<&ResourceRequest>,
        declared: &ResourceRequest,
    ) -> Result<Self> {
        let request = admitted.unwrap_or(declared);
        let memory_mib = match admitted.map(|admitted| admitted.memory_bytes) {
            Some(bytes) if bytes > 0 => {
                let whole = bytes / MIB;
                if whole == 0 {
                    return Err(Error::RemoteProviderConfig {
                        provider: "microsandbox".to_string(),
                        message: format!(
                            "the admitted host memory of {bytes} bytes is below the 1 MiB a \
                             virtual machine needs; raise --memory-limit or the action's \
                             memory estimate"
                        ),
                    });
                }
                whole
            }
            _ => match declared.memory_bytes {
                0 => DEFAULT_ACTION_MEMORY_BYTES,
                bytes => bytes,
            }
            .div_ceil(MIB),
        };
        Ok(Self {
            cpus: u8::try_from(request.cpu_slots.clamp(1, usize::from(u8::MAX))).unwrap_or(u8::MAX),
            memory_mib: u32::try_from(memory_mib).unwrap_or(u32::MAX),
        })
    }
}

/// Stop a guest gracefully, escalating to `kill` when the stop fails or does
/// not finish in time. Both phases are bounded, so cleanup always ends; an
/// error means the guest may still be running.
pub(super) async fn stop_or_kill<Stop, Kill, Killed>(
    name: &str,
    stop: Stop,
    kill: Kill,
    stop_timeout: Duration,
    kill_timeout: Duration,
) -> Result<()>
where
    Stop: Future<Output = Result<()>>,
    Kill: FnOnce() -> Killed,
    Killed: Future<Output = Result<()>>,
{
    match tokio::time::timeout(stop_timeout, stop).await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(error)) => {
            tracing::warn!(provider = "microsandbox", sandbox = name, %error, "graceful stop failed; killing sandbox");
        }
        Err(_) => {
            tracing::warn!(
                provider = "microsandbox",
                sandbox = name,
                timeout_ms = u64::try_from(stop_timeout.as_millis()).unwrap_or(u64::MAX),
                "graceful stop timed out; killing sandbox"
            );
        }
    }
    match tokio::time::timeout(kill_timeout, kill()).await {
        Ok(result) => result,
        Err(_) => Err(Error::RemoteProviderApi {
            provider: "microsandbox".to_string(),
            message: format!(
                "sandbox {name} did not exit within {}s of being killed and may still be running",
                kill_timeout.as_secs()
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn failure() -> Error {
        Error::RemoteProviderApi {
            provider: "microsandbox".to_string(),
            message: "boom".to_string(),
        }
    }

    fn size(admitted: Option<&ResourceRequest>, declared: &ResourceRequest) -> GuestSize {
        GuestSize::for_request(admitted, declared).unwrap()
    }

    #[test]
    fn guest_matches_the_admitted_reservation() {
        let declared = ResourceRequest::new(8, 0);
        let admitted = ResourceRequest::new(2, 300 * MIB);
        assert_eq!(
            size(Some(&admitted), &declared),
            GuestSize {
                cpus: 2,
                memory_mib: 300
            }
        );
    }

    #[test]
    fn fractional_admitted_memory_rounds_down_to_stay_within_the_charge() {
        for (bytes, expected) in [(250 * MIB + 1, 250), (2 * MIB - 1, 1), (MIB, 1)] {
            let admitted = ResourceRequest::new(1, bytes);
            let guest = size(Some(&admitted), &ResourceRequest::default());
            assert_eq!(guest.memory_mib, expected);
            assert!(u64::from(guest.memory_mib) * MIB <= bytes);
        }
    }

    #[test]
    fn admitted_memory_below_one_mib_cannot_host_a_guest() {
        for bytes in [1, MIB - 1] {
            let admitted = ResourceRequest::new(1, bytes);
            let error =
                GuestSize::for_request(Some(&admitted), &ResourceRequest::default()).unwrap_err();
            assert!(error.to_string().contains("below the 1 MiB"));
        }
    }

    #[test]
    fn unadmitted_guests_use_the_declared_or_generic_estimate() {
        assert_eq!(
            size(None, &ResourceRequest::new(3, 700 * MIB)),
            GuestSize {
                cpus: 3,
                memory_mib: 700
            }
        );
        assert_eq!(
            size(None, &ResourceRequest::default()).memory_mib,
            u32::try_from(DEFAULT_ACTION_MEMORY_BYTES / MIB).unwrap()
        );
    }

    #[test]
    fn a_pool_without_a_memory_axis_still_sizes_the_guest() {
        let admitted = ResourceRequest::new(1, 0);
        assert_eq!(
            size(Some(&admitted), &ResourceRequest::new(1, 64 * MIB)).memory_mib,
            64
        );
        assert_eq!(
            size(Some(&admitted), &ResourceRequest::default()).memory_mib,
            u32::try_from(DEFAULT_ACTION_MEMORY_BYTES / MIB).unwrap()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_clean_stop_never_kills() {
        let killed = AtomicBool::new(false);
        let result = stop_or_kill(
            "guest",
            async { Ok(()) },
            || async {
                killed.store(true, Ordering::SeqCst);
                Ok(())
            },
            STOP_TIMEOUT,
            KILL_TIMEOUT,
        )
        .await;
        assert!(result.is_ok());
        assert!(!killed.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_or_failed_stop_escalates_to_kill() {
        for hangs in [true, false] {
            let killed = AtomicBool::new(false);
            let stop = async move {
                if hangs {
                    std::future::pending::<()>().await;
                }
                Err(failure())
            };
            let result = stop_or_kill(
                "guest",
                stop,
                || async {
                    killed.store(true, Ordering::SeqCst);
                    Ok(())
                },
                STOP_TIMEOUT,
                KILL_TIMEOUT,
            )
            .await;
            assert!(result.is_ok());
            assert!(killed.load(Ordering::SeqCst));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_unkillable_guest_ends_cleanup_with_an_error() {
        let started = tokio::time::Instant::now();
        let error = stop_or_kill(
            "guest",
            std::future::pending::<Result<()>>(),
            std::future::pending::<Result<()>>,
            STOP_TIMEOUT,
            KILL_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("may still be running"));
        assert_eq!(started.elapsed(), STOP_TIMEOUT + KILL_TIMEOUT);
    }
}
