//! Builds the command for a local action, wrapping it in the platform's
//! network isolation when the action declared `network = "deny"`.
//!
//! Denial fails closed: a platform or architecture without an enforcement
//! mechanism refuses to run the action instead of running it with the
//! network reachable.

use tokio::process::Command;

use crate::{NetworkPolicy, Result};

/// The Seatbelt profile for a denied action. Every network operation is
/// denied, loopback included, and then `AF_UNIX` sockets are allowed back so
/// local inter-process communication keeps working. The system DNS resolver
/// is reached over a Unix socket and would resolve names on the action's
/// behalf, so it stays denied.
#[cfg(target_os = "macos")]
pub(super) const DENY_NETWORK_PROFILE: &str = concat!(
    "(version 1)",
    " (allow default)",
    " (deny network*)",
    " (allow network* (local unix-socket))",
    " (allow network* (remote unix-socket))",
    " (deny network-outbound (remote unix-socket (path-literal \"/private/var/run/mDNSResponder\")))",
);

/// The command that runs `program` under `network`.
pub(super) fn command(network: NetworkPolicy, program: &str, args: &[String]) -> Result<Command> {
    if !network.is_denied() {
        let mut command = Command::new(program);
        command.args(args);
        return Ok(command);
    }
    deny_network(program, args)
}

#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)]
fn deny_network(program: &str, args: &[String]) -> Result<Command> {
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.args(["-p", DENY_NETWORK_PROFILE, "--", program]);
    command.args(args);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn deny_network(program: &str, args: &[String]) -> Result<Command> {
    let mut command = Command::new(program);
    command.args(args);
    match crate::network::arm(&mut command) {
        Ok(()) => Ok(command),
        Err(requirement) => Err(unenforceable(program, requirement)),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn deny_network(program: &str, _args: &[String]) -> Result<Command> {
    Err(unenforceable(program, "network denial on this platform"))
}

#[cfg(not(target_os = "macos"))]
fn unenforceable(program: &str, requirement: &'static str) -> crate::Error {
    tracing::warn!(
        program = %program,
        requirement,
        "network `deny` cannot be enforced here; refusing to run the action"
    );
    crate::Executor::Local.unsupported(requirement)
}
