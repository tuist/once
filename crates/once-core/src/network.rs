//! Seccomp filter that denies network syscalls for an action that declared
//! [`NetworkPolicy::Deny`](crate::NetworkPolicy::Deny).
//!
//! Linux only. The filter is built in the parent process, where allocation is
//! fine, then installed in the child through `CommandExt::pre_exec` after
//! fork. The install path calls only async-signal-safe syscalls (`prctl`), so
//! it is safe to run between fork and exec. Every denied syscall number comes
//! from `libc::SYS_*`, which the libc crate resolves per architecture, so the
//! filter needs no hardcoded numbers and works on both x86_64 and aarch64.

#[cfg(target_os = "linux")]
mod filter {
    use std::{io, ptr};

    /// Network-related syscalls a denied action must not be able to issue.
    ///
    /// `socketpair` is deliberately absent: it can only create AF_UNIX pairs,
    /// which tools use for local IPC, and it never touches the network. The
    /// set covers everything else from creating a socket to exchanging data,
    /// so a denied action cannot reach the network even through a server that
    /// binds and accepts.
    const DENIED: &[libc::c_long] = &[
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_shutdown,
    ];

    // BPF instruction encodings for a seccomp filter. The numeric opcodes are
    // stable Linux UAPI and never change.
    const LD_NR: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS, load seccomp_data.nr
    const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    const RET_K: u16 = 0x06; // BPF_RET | BPF_K

    fn stmt(code: u16, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }

    /// Build the seccomp BPF program that denies every syscall in `DENIED`,
    /// returning `EACCES`, and allows everything else. Split out from
    /// [`install`] so the program shape can be inspected without running it.
    #[must_use]
    pub(crate) fn deny_filter() -> Vec<libc::sock_filter> {
        let m = DENIED.len();
        let mut prog = Vec::with_capacity(m + 3);
        prog.push(stmt(LD_NR, 0));
        for (i, sys) in DENIED.iter().enumerate() {
            // On match, jump forward over the remaining checks and the ALLOW
            // return to the ERRNO return at the end. BPF jump offsets are
            // relative to the next instruction; for the (i+1)th check, that is
            // `m - i` instructions past it.
            let jump_to_errno = (m - i) as u8;
            prog.push(libc::sock_filter {
                code: JEQ_K,
                jt: jump_to_errno,
                jf: 0,
                k: *sys as u32,
            });
        }
        prog.push(stmt(RET_K, libc::SECCOMP_RET_ALLOW));
        prog.push(stmt(RET_K, libc::SECCOMP_RET_ERRNO | (libc::EACCES as u32)));
        prog
    }

    /// Drop privileges and apply the filter. Must run in the child after fork.
    ///
    /// # Safety
    /// Calls `prctl`, which is async-signal-safe and the only thing this
    /// function does between fork and exec.
    pub(crate) unsafe fn install(filter: &[libc::sock_filter]) -> io::Result<()> {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        let prog = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_ptr() as *mut _,
        };
        if libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            ptr::addr_of!(prog),
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn filter_loads_then_checks_then_allows() {
            let prog = deny_filter();
            // LD, one check per denied syscall, ALLOW, ERRNO.
            assert_eq!(prog.len(), DENIED.len() + 3);
            assert_eq!(prog[0].code, LD_NR);
            // The final two instructions are the returns.
            assert_eq!(prog[prog.len() - 2].code, RET_K);
            assert_eq!(prog[prog.len() - 2].k, libc::SECCOMP_RET_ALLOW);
            assert_eq!(
                prog[prog.len() - 1].k,
                libc::SECCOMP_RET_ERRNO | libc::EACCES as u32
            );
        }

        #[test]
        fn first_check_jumps_straight_to_errno() {
            let prog = deny_filter();
            let first_check = &prog[1];
            // First check (i = 0) jumps `m` instructions: over the remaining
            // m-1 checks and the ALLOW return, landing on ERRNO.
            assert_eq!(first_check.jt as usize, DENIED.len());
            assert_eq!(first_check.jf, 0);
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use filter::{deny_filter, install};
