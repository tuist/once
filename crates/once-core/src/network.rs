//! Seccomp filter that denies network access for an action that declared
//! [`NetworkPolicy::Deny`](crate::NetworkPolicy::Deny).
//!
//! Linux only. The filter is built in the parent process, where allocation is
//! fine, then installed in the child through `CommandExt::pre_exec` after
//! fork. Everything that runs in the child is a raw, async-signal-safe
//! syscall (`fstat`, `getsockopt`, `close_range`, `prctl`) on memory prepared
//! before fork, so it is safe to run between fork and exec.
//!
//! The guarantee is that the action never holds a socket of any family other
//! than `AF_UNIX`. Three ways to hold one are closed:
//!
//! - Creating it: `socket` and `socketpair` succeed only for `AF_UNIX`, so
//!   every other address family, loopback included, fails with `EACCES`.
//!   Syscalls that reach the network without going through `socket`
//!   (`io_uring`, which has its own socket and connect operations) are
//!   unavailable.
//! - Inheriting it: before exec, every descriptor above stderr is marked
//!   close-on-exec, so a socket Once itself inherited without `O_CLOEXEC`
//!   does not reach the action. The standard streams must be files, pipes,
//!   or `AF_UNIX` sockets; any other socket there fails the spawn with
//!   `EAFNOSUPPORT`. A kernel without `close_range` and its close-on-exec
//!   flag (Linux 5.11) cannot do this sweep, so [`arm`] refuses it.
//! - Receiving it: descriptors arrive from another process only through
//!   `SCM_RIGHTS` ancillary data or `pidfd_getfd`, so `recvmsg`, `recvmmsg`,
//!   and `pidfd_getfd` fail with `EACCES` (with `sendmmsg` for symmetry).
//!   This restricts Unix-socket IPC to plain reads and writes: tools whose
//!   IPC channel passes descriptors or always uses `recvmsg`, such as
//!   libuv's IPC pipes behind Node's `child_process.fork`, fail under a
//!   denied network.
//!
//! Calls that act on an existing socket (`connect`, `sendto`, `read`, ...)
//! stay allowed because the only sockets left are `AF_UNIX`. This does not
//! stop an action from asking a process outside the sandbox to reach the
//! network on its behalf over a Unix socket.
//!
//! The filter first checks the syscall ABI: a syscall issued through any
//! architecture other than the native one (the 32-bit `int 0x80` entry on
//! `x86_64`), or through the `x32` ABI, kills the process, because syscall
//! numbers differ between ABIs and the native table would not cover it.
//! Architectures without a known audit identifier are not armed at all;
//! [`arm`] reports them so the caller fails closed.
//!
//! The kernel must provide `PR_SET_NO_NEW_PRIVS` (Linux 3.5), seccomp filter
//! mode (`CONFIG_SECCOMP_FILTER`), and `close_range` with
//! `CLOSE_RANGE_CLOEXEC` (Linux 5.11). [`arm`] probes each in the parent and
//! names the first one missing, so an unenforceable denial is refused before
//! fork rather than surfacing as an opaque spawn error. Landlock and network
//! namespaces are not used.

mod program;

#[cfg(test)]
mod tests;

// This module is the workspace's sanctioned FFI-boundary use of `unsafe`: it
// installs a seccomp filter through `prctl` and `pre_exec`, which cannot be
// expressed in safe Rust. Scoped here so the rest of the crate stays under the
// workspace-wide `unsafe_code = "deny"`.
#[allow(unsafe_code)]
mod install {
    use std::io;

    use tokio::process::Command;

    use super::program::{self, Instruction};

    fn to_sock_filter(instruction: Instruction) -> libc::sock_filter {
        libc::sock_filter {
            code: instruction.code,
            jt: instruction.jt,
            jf: instruction.jf,
            k: instruction.k,
        }
    }

    /// Fail unless `fd`, one of the standard streams, is closed, not a
    /// socket, or an `AF_UNIX` socket.
    ///
    /// # Safety
    /// Calls only `fstat` and `getsockopt` on stack memory; async-signal-safe.
    unsafe fn check_standard_stream(fd: libc::c_int) -> io::Result<()> {
        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &raw mut stat) != 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EBADF) {
                Ok(())
            } else {
                Err(error)
            };
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return Ok(());
        }
        let mut domain: libc::c_int = 0;
        let mut len = libc::socklen_t::try_from(std::mem::size_of::<libc::c_int>())
            .expect("an int fits in socklen_t");
        if libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_DOMAIN,
            (&raw mut domain).cast(),
            &raw mut len,
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        if domain == libc::AF_UNIX {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(libc::EAFNOSUPPORT))
        }
    }

    /// Mark every descriptor from `first` up close-on-exec. The descriptors
    /// stay open until exec, so the pipe the standard library uses to report
    /// a failed exec (already close-on-exec) keeps working.
    ///
    /// # Safety
    /// A single raw syscall; async-signal-safe.
    unsafe fn close_on_exec_from(first: libc::c_uint) -> io::Result<()> {
        if libc::syscall(
            libc::SYS_close_range,
            first,
            libc::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        ) == 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Whether this kernel supports `close_range` with `CLOSE_RANGE_CLOEXEC`.
    /// The probed range holds no descriptors, so nothing is touched; older
    /// kernels fail with `ENOSYS` or `EINVAL`, and a container seccomp
    /// profile that does not know the syscall fails it too.
    fn close_on_exec_is_supported() -> bool {
        // SAFETY: the range starts above every valid descriptor.
        unsafe { close_on_exec_from(libc::c_uint::MAX).is_ok() }
    }

    /// Whether this kernel accepts seccomp filters. Installing a null filter
    /// pointer fails with `EFAULT` only after the kernel has recognized
    /// filter mode, so nothing is installed; a kernel built without
    /// `CONFIG_SECCOMP_FILTER`, or a runtime that intercepts the call, fails
    /// it some other way.
    fn seccomp_filter_is_supported() -> bool {
        // SAFETY: a null filter pointer is rejected before anything is
        // installed.
        let rc = unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::null::<libc::sock_fprog>(),
            )
        };
        rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::EFAULT)
    }

    /// Whether this kernel supports `PR_SET_NO_NEW_PRIVS` (Linux 3.5), which
    /// unprivileged seccomp filters require. Reading the flag changes nothing.
    fn no_new_privs_is_supported() -> bool {
        // SAFETY: `PR_GET_NO_NEW_PRIVS` only reads the calling thread's flag.
        unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) >= 0 }
    }

    /// Check the standard streams, keep inherited descriptors from reaching
    /// the action, then drop privileges and apply the filter. Must run in the
    /// child after fork, after the standard streams are in place.
    ///
    /// # Safety
    /// Issues only async-signal-safe syscalls against memory prepared before
    /// fork; it allocates nothing.
    unsafe fn isolate(prog: &libc::sock_fprog) -> io::Result<()> {
        for fd in 0..=2 {
            check_standard_stream(fd)?;
        }
        close_on_exec_from(3)?;
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            std::ptr::from_ref(prog),
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Arrange for `command` to isolate its child from the network after
    /// fork. Leaves the command untouched and returns the requirement this
    /// host cannot meet, so the caller can refuse to run instead of running
    /// with the network reachable.
    pub(crate) fn arm(command: &mut Command) -> Result<(), &'static str> {
        let Some(abi) = program::Abi::native() else {
            return Err("network denial on this platform");
        };
        if !no_new_privs_is_supported() {
            return Err(
                "network denial on this kernel, which lacks prctl PR_SET_NO_NEW_PRIVS (Linux 3.5)",
            );
        }
        if !seccomp_filter_is_supported() {
            return Err(
                "network denial on this kernel, which lacks seccomp filter mode (CONFIG_SECCOMP_FILTER) or whose runtime blocks prctl PR_SET_SECCOMP",
            );
        }
        if !close_on_exec_is_supported() {
            return Err(
                "network denial on this kernel, which lacks close_range with CLOSE_RANGE_CLOEXEC (Linux 5.11)",
            );
        }
        let filter = program::deny_network(&abi)
            .into_iter()
            .map(to_sock_filter)
            .collect::<Vec<_>>();
        let Ok(len) = u16::try_from(filter.len()) else {
            return Err("network denial with a seccomp filter this long");
        };
        // SAFETY: `pre_exec` runs in the child after fork; `isolate` issues
        // only async-signal-safe syscalls against the filter built here in
        // the parent and moved into the closure, which outlives the call.
        unsafe {
            command.pre_exec(move || {
                let prog = libc::sock_fprog {
                    len,
                    filter: filter.as_ptr().cast_mut(),
                };
                isolate(&prog)
            });
        }
        Ok(())
    }
}

pub(crate) use install::arm;
