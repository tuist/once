//! Waits for a local action while its output is captured, enforcing the
//! timeout and bounding how long output pipes may outlive the leader.

use std::future::Future;
use std::process::ExitStatus;
use std::time::Duration;

use struo_cas::Digest;
use tracing::{debug, warn};

use super::process_group::GroupChild;
use crate::{Error, Result};

/// How long output pipes may stay open after the leader exits, and how long a
/// killed group gets to release them. A background process that inherited
/// stdout or stderr would otherwise keep the capture, and the action, open
/// for as long as it lives.
pub(super) const DRAIN_GRACE: Duration = Duration::from_secs(2);

pub(super) type Captured = (Option<Digest>, Option<Digest>);

enum Event {
    Exited(std::io::Result<()>),
    Captured(Result<Captured>),
    TimedOut,
}

/// Drive `capture` and the leader to completion.
///
/// The leader is reaped only after the action is done with its group, so every
/// kill below addresses the action's own group. On timeout the whole group is
/// killed. When the leader exits on its own, its pipes get [`DRAIN_GRACE`] to
/// close; if something in the group still holds them after that, the group is
/// killed and given the same grace again before the capture is abandoned.
pub(super) async fn supervise(
    mut group: GroupChild,
    program: &str,
    timeout_ms: Option<u64>,
    capture: impl Future<Output = Result<Captured>>,
) -> Result<(ExitStatus, Captured)> {
    tokio::pin!(capture);
    let timeout = timeout_ms.map(Duration::from_millis);
    let deadline = async {
        match timeout {
            Some(timeout) => tokio::time::sleep(timeout).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(deadline);

    let mut captured = None;
    loop {
        let event = tokio::select! {
            biased;
            exited = group.exited() => Event::Exited(exited),
            result = &mut capture, if captured.is_none() => Event::Captured(result),
            () = &mut deadline => Event::TimedOut,
        };
        match event {
            Event::Exited(exited) => {
                exited.map_err(|source| Error::Wait {
                    program: program.to_string(),
                    source,
                })?;
                break;
            }
            Event::Captured(result) => captured = Some(result?),
            Event::TimedOut => {
                let timeout = timeout.expect("only a configured deadline fires");
                debug!(
                    program = %program,
                    timeout_ms = timeout.as_millis(),
                    "local command timed out; killing its process group"
                );
                group.kill_group();
                if tokio::time::timeout(DRAIN_GRACE, group.exited())
                    .await
                    .is_ok_and(|exited| exited.is_ok())
                {
                    let _ = group.reap().await;
                }
                return Err(Error::Timeout(timeout));
            }
        }
    }

    let captured = match captured {
        Some(captured) => captured,
        None => drain(&mut group, program, &mut capture).await?,
    };
    let status = group.reap().await.map_err(|source| Error::Wait {
        program: program.to_string(),
        source,
    })?;
    Ok((status, captured))
}

async fn drain(
    group: &mut GroupChild,
    program: &str,
    capture: &mut (impl Future<Output = Result<Captured>> + Unpin),
) -> Result<Captured> {
    if let Ok(result) = tokio::time::timeout(DRAIN_GRACE, &mut *capture).await {
        return result;
    }
    warn!(
        program = %program,
        grace_ms = DRAIN_GRACE.as_millis(),
        "output stayed open after the command exited; killing the processes still holding it"
    );
    group.kill_group();
    match tokio::time::timeout(DRAIN_GRACE, capture).await {
        Ok(result) => result,
        Err(_) => Err(Error::Wait {
            program: program.to_string(),
            source: std::io::Error::other(
                "output stayed open after the command exited and its process group was killed; \
                 a process outside the group still holds it",
            ),
        }),
    }
}
