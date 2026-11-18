//! The seccomp BPF program, built from an [`Abi`] description so its shape can
//! be evaluated in tests against any syscall table.

/// One classic BPF instruction, laid out like `struct sock_filter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Instruction {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

// BPF opcodes, seccomp return actions, and the errno values the filter
// returns are stable Linux UAPI and identical on every supported
// architecture.
pub(crate) const LD_W_ABS: u16 = 0x20;
pub(crate) const JEQ_K: u16 = 0x15;
pub(crate) const JSET_K: u16 = 0x45;
pub(crate) const RET_K: u16 = 0x06;

pub(crate) const RET_ALLOW: u32 = 0x7fff_0000;
pub(crate) const RET_ERRNO: u32 = 0x0005_0000;
pub(crate) const RET_KILL_PROCESS: u32 = 0x8000_0000;

pub(crate) const EACCES: u32 = 13;
pub(crate) const ENOSYS: u32 = 38;
pub(crate) const AF_UNIX: u32 = 1;

/// Offsets into `struct seccomp_data`. `args[0]` is a `u64`; both supported
/// architectures are little-endian, so its low word, which holds the `int`
/// address family, sits at the start of the field.
pub(crate) const NR_OFFSET: u32 = 0;
pub(crate) const ARCH_OFFSET: u32 = 4;
pub(crate) const ARG0_LOW_OFFSET: u32 = 16;

/// The syscall numbers and audit identifier of one architecture.
#[derive(Clone, Debug)]
pub(crate) struct Abi {
    /// `AUDIT_ARCH_*` of the native syscall entry.
    pub arch: u32,
    /// Bit that marks a syscall number as coming from an alternate ABI that
    /// shares the native audit identifier (`__X32_SYSCALL_BIT` on `x86_64`).
    pub alternate_abi_bit: Option<u32>,
    pub socket: u32,
    pub socketpair: u32,
    /// Denied with `EACCES`.
    pub denied: Vec<u32>,
    /// Denied with `ENOSYS`, so callers that probe for the facility fall back
    /// to plain syscalls the filter can inspect.
    pub unavailable: Vec<u32>,
}

impl Abi {
    /// The table for the architecture this binary targets, or `None` where
    /// Struo has no audit identifier for it.
    #[cfg(target_os = "linux")]
    pub(crate) fn native() -> Option<Self> {
        let arch = NATIVE_AUDIT_ARCH?;
        let number = |sys: libc::c_long| u32::try_from(sys).expect("syscall numbers fit in u32");
        Some(Self {
            arch,
            alternate_abi_bit: ALTERNATE_ABI_BIT,
            socket: number(libc::SYS_socket),
            socketpair: number(libc::SYS_socketpair),
            denied: [
                libc::SYS_recvmsg,
                libc::SYS_sendmmsg,
                libc::SYS_recvmmsg,
                libc::SYS_pidfd_getfd,
            ]
            .into_iter()
            .map(number)
            .collect(),
            unavailable: [
                libc::SYS_io_uring_setup,
                libc::SYS_io_uring_enter,
                libc::SYS_io_uring_register,
            ]
            .into_iter()
            .map(number)
            .collect(),
        })
    }
}

/// `AUDIT_ARCH_*` of the native syscall entry: the ELF machine with the
/// 64-bit and little-endian flags.
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
))]
const NATIVE_AUDIT_ARCH: Option<u32> = Some(0xc000_003e);
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const NATIVE_AUDIT_ARCH: Option<u32> = Some(0xc000_00b7);
#[cfg(all(
    target_os = "linux",
    not(any(
        all(target_arch = "x86_64", target_pointer_width = "64"),
        target_arch = "aarch64"
    ))
))]
const NATIVE_AUDIT_ARCH: Option<u32> = None;

/// `__X32_SYSCALL_BIT`: x32 syscalls enter through the `x86_64` audit
/// identifier with this bit set in the syscall number.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const ALTERNATE_ABI_BIT: Option<u32> = Some(0x4000_0000);
#[cfg(all(target_os = "linux", not(target_arch = "x86_64")))]
const ALTERNATE_ABI_BIT: Option<u32> = None;

fn stmt(code: u16, k: u32) -> Instruction {
    Instruction {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Instruction {
    Instruction { code, jt, jf, k }
}

/// Build the program that denies network access for `abi`.
///
/// Every check either returns or falls through to the next one, and each jump
/// is at most a few instructions forward, so offsets never depend on how many
/// syscalls are denied.
pub(crate) fn deny_network(abi: &Abi) -> Vec<Instruction> {
    let mut program = vec![
        stmt(LD_W_ABS, ARCH_OFFSET),
        jump(JEQ_K, abi.arch, 1, 0),
        stmt(RET_K, RET_KILL_PROCESS),
        stmt(LD_W_ABS, NR_OFFSET),
    ];
    if let Some(bit) = abi.alternate_abi_bit {
        program.push(jump(JSET_K, bit, 0, 1));
        program.push(stmt(RET_K, RET_KILL_PROCESS));
    }
    program.extend([
        jump(JEQ_K, abi.socket, 1, 0),
        jump(JEQ_K, abi.socketpair, 0, 4),
        stmt(LD_W_ABS, ARG0_LOW_OFFSET),
        jump(JEQ_K, AF_UNIX, 0, 1),
        stmt(RET_K, RET_ALLOW),
        stmt(RET_K, RET_ERRNO | EACCES),
    ]);
    for (syscalls, errno) in [(&abi.denied, EACCES), (&abi.unavailable, ENOSYS)] {
        for &syscall in syscalls {
            program.push(jump(JEQ_K, syscall, 0, 1));
            program.push(stmt(RET_K, RET_ERRNO | errno));
        }
    }
    program.push(stmt(RET_K, RET_ALLOW));
    program
}
