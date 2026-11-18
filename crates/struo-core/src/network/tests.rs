use super::program::{
    deny_network, Abi, Instruction, AF_UNIX, ARCH_OFFSET, ARG0_LOW_OFFSET, EACCES, ENOSYS, JEQ_K,
    JSET_K, LD_W_ABS, NR_OFFSET, RET_ALLOW, RET_ERRNO, RET_K, RET_KILL_PROCESS,
};

const ARCH: u32 = 0xc000_003e;
const AUDIT_ARCH_I386: u32 = 0x4000_0003;
const X32_BIT: u32 = 0x4000_0000;
const SOCKET: u32 = 41;
const SOCKETPAIR: u32 = 53;
const SENDMMSG: u32 = 307;
const RECVMSG: u32 = 47;
const RECVMMSG: u32 = 299;
const PIDFD_GETFD: u32 = 438;
const SENDMSG: u32 = 46;
const READ: u32 = 0;
const IO_URING_SETUP: u32 = 425;
const CONNECT: u32 = 42;
const WRITE: u32 = 1;
const AF_INET: u32 = 2;
const AF_INET6: u32 = 10;
const AF_NETLINK: u32 = 16;

fn abi(alternate_abi_bit: Option<u32>) -> Abi {
    Abi {
        arch: ARCH,
        alternate_abi_bit,
        socket: SOCKET,
        socketpair: SOCKETPAIR,
        denied: vec![RECVMSG, SENDMMSG, RECVMMSG, PIDFD_GETFD],
        unavailable: vec![IO_URING_SETUP],
    }
}

/// Evaluate a seccomp program the way the kernel does for the subset of
/// classic BPF the filter uses.
fn evaluate(program: &[Instruction], arch: u32, nr: u32, arg0: u64) -> u32 {
    let load = |offset: u32| match offset {
        NR_OFFSET => nr,
        ARCH_OFFSET => arch,
        ARG0_LOW_OFFSET => u32::try_from(arg0 & 0xffff_ffff).unwrap(),
        other => panic!("unexpected load offset {other}"),
    };
    let mut accumulator = 0;
    let mut pc = 0;
    loop {
        let instruction = program[pc];
        pc += 1;
        match instruction.code {
            LD_W_ABS => accumulator = load(instruction.k),
            RET_K => return instruction.k,
            JEQ_K | JSET_K => {
                let taken = if instruction.code == JEQ_K {
                    accumulator == instruction.k
                } else {
                    accumulator & instruction.k != 0
                };
                pc += usize::from(if taken {
                    instruction.jt
                } else {
                    instruction.jf
                });
            }
            other => panic!("unexpected opcode {other:#x}"),
        }
    }
}

#[test]
fn only_unix_sockets_can_be_created() {
    let program = deny_network(&abi(Some(X32_BIT)));
    for syscall in [SOCKET, SOCKETPAIR] {
        assert_eq!(
            evaluate(&program, ARCH, syscall, u64::from(AF_UNIX)),
            RET_ALLOW
        );
        for family in [AF_INET, AF_INET6, AF_NETLINK, 0, u32::MAX] {
            assert_eq!(
                evaluate(&program, ARCH, syscall, u64::from(family)),
                RET_ERRNO | EACCES,
                "syscall {syscall} family {family}"
            );
        }
    }
}

#[test]
fn the_family_is_read_from_the_low_word_the_kernel_uses() {
    let program = deny_network(&abi(None));
    let high_bits_with_inet = (1_u64 << 32) | u64::from(AF_INET);
    assert_eq!(
        evaluate(&program, ARCH, SOCKET, high_bits_with_inet),
        RET_ERRNO | EACCES
    );
}

#[test]
fn foreign_architectures_are_killed() {
    let program = deny_network(&abi(Some(X32_BIT)));
    for nr in [SOCKET, WRITE, 102] {
        assert_eq!(
            evaluate(&program, AUDIT_ARCH_I386, nr, u64::from(AF_INET)),
            RET_KILL_PROCESS
        );
    }
}

#[test]
fn alternate_abi_syscalls_are_killed() {
    let program = deny_network(&abi(Some(X32_BIT)));
    assert_eq!(
        evaluate(&program, ARCH, X32_BIT | SOCKET, u64::from(AF_INET)),
        RET_KILL_PROCESS
    );
    assert_eq!(
        evaluate(&program, ARCH, X32_BIT | WRITE, 0),
        RET_KILL_PROCESS
    );
}

#[test]
fn bypass_syscalls_are_denied_and_the_rest_allowed() {
    let program = deny_network(&abi(Some(X32_BIT)));
    assert_eq!(evaluate(&program, ARCH, SENDMMSG, 0), RET_ERRNO | EACCES);
    assert_eq!(
        evaluate(&program, ARCH, IO_URING_SETUP, 0),
        RET_ERRNO | ENOSYS
    );
    assert_eq!(evaluate(&program, ARCH, CONNECT, 0), RET_ALLOW);
    assert_eq!(evaluate(&program, ARCH, WRITE, 0), RET_ALLOW);
}

#[test]
fn descriptors_cannot_be_received_from_other_processes() {
    let program = deny_network(&abi(Some(X32_BIT)));
    for syscall in [RECVMSG, RECVMMSG, PIDFD_GETFD] {
        assert_eq!(
            evaluate(&program, ARCH, syscall, 0),
            RET_ERRNO | EACCES,
            "syscall {syscall}"
        );
    }
    for syscall in [READ, WRITE, SENDMSG] {
        assert_eq!(evaluate(&program, ARCH, syscall, 0), RET_ALLOW);
    }
}

#[test]
fn uapi_constants_match_libc() {
    assert_eq!(RET_ALLOW, libc::SECCOMP_RET_ALLOW);
    assert_eq!(RET_ERRNO, libc::SECCOMP_RET_ERRNO);
    assert_eq!(RET_KILL_PROCESS, libc::SECCOMP_RET_KILL_PROCESS);
    assert_eq!(EACCES, u32::try_from(libc::EACCES).unwrap());
    assert_eq!(ENOSYS, u32::try_from(libc::ENOSYS).unwrap());
    assert_eq!(AF_UNIX, u32::try_from(libc::AF_UNIX).unwrap());
}

#[test]
fn the_native_table_is_known_on_supported_architectures() {
    let native = Abi::native();
    if cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        let native = native.expect("supported architecture has a table");
        assert_eq!(
            native.alternate_abi_bit.is_some(),
            cfg!(target_arch = "x86_64")
        );
        assert_eq!(native.socket, u32::try_from(libc::SYS_socket).unwrap());
        for syscall in [libc::SYS_recvmsg, libc::SYS_recvmmsg, libc::SYS_pidfd_getfd] {
            assert!(native.denied.contains(&u32::try_from(syscall).unwrap()));
        }
    } else {
        assert!(native.is_none());
    }
}

mod live {
    use std::process::Stdio;

    use tokio::process::Command;

    const CHILD_MODE: &str = "STRUO_NETWORK_FILTER_TEST_CHILD";
    const INHERITED_FD: &str = "STRUO_NETWORK_FILTER_TEST_FD";

    /// Probes the filter from inside a filtered process. Re-executed by the
    /// tests below because the filter cannot be removed once installed.
    #[test]
    #[ignore = "run by the tests below in a filtered child process"]
    fn child() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        match mode.as_str() {
            "probe" => probe(),
            "x32" => alternate_abi_socket(),
            "inherited" => {
                let fd = std::env::var(INHERITED_FD).unwrap().parse().unwrap();
                report("inherited", raw::descriptor_is_open(fd));
            }
            "noop" => println!("started=ok"),
            other => panic!("unknown mode {other}"),
        }
    }

    fn report(name: &str, result: std::io::Result<()>) {
        match result {
            Ok(()) => println!("{name}=ok"),
            Err(error) => println!("{name}=errno:{}", error.raw_os_error().unwrap_or(-1)),
        }
    }

    fn probe() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};

        report("tcp4", std::net::TcpListener::bind("127.0.0.1:0").map(drop));
        report("tcp6", std::net::TcpListener::bind("[::1]:0").map(drop));
        report("udp4", std::net::UdpSocket::bind("127.0.0.1:0").map(drop));
        report(
            "connect4",
            std::net::TcpStream::connect("127.0.0.1:9").map(drop),
        );
        report(
            "unix_stream",
            (|| {
                let dir = tempfile::tempdir()?;
                let path = dir.path().join("socket");
                let listener = UnixListener::bind(&path)?;
                let mut client = UnixStream::connect(&path)?;
                let (mut server, _) = listener.accept()?;
                client.write_all(b"hi")?;
                let mut buffer = [0; 2];
                server.read_exact(&mut buffer)?;
                assert_eq!(&buffer, b"hi");
                Ok(())
            })(),
        );
        report("unix_pair", UnixStream::pair().map(drop));
        report("unix_datagram_pair", UnixDatagram::pair().map(drop));
        report("netlink", raw::socket(libc::AF_NETLINK));
        report("io_uring_setup", raw::io_uring_setup());
        report("sendmmsg", raw::sendmmsg());
        report(
            "recvmsg",
            UnixStream::pair().and_then(|(mut sender, receiver)| {
                sender.write_all(b"x")?;
                raw::recvmsg(&receiver)
            }),
        );
    }

    fn alternate_abi_socket() {
        let result = raw::x32_socket();
        println!("x32_survived={result:?}");
    }

    // Probing syscalls that have no safe wrapper needs raw `syscall`.
    #[allow(unsafe_code)]
    mod raw {
        fn check(rc: libc::c_long) -> std::io::Result<()> {
            if rc < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub(super) fn socket(family: libc::c_int) -> std::io::Result<()> {
            // SAFETY: `socket` takes plain integers.
            let fd = unsafe { libc::socket(family, libc::SOCK_RAW, 0) };
            check(libc::c_long::from(fd))
        }

        pub(super) fn io_uring_setup() -> std::io::Result<()> {
            let mut params = [0_u8; 120];
            // SAFETY: `params` is a writable buffer at least as large as
            // `struct io_uring_params`.
            check(unsafe { libc::syscall(libc::SYS_io_uring_setup, 1, params.as_mut_ptr()) })
        }

        pub(super) fn sendmmsg() -> std::io::Result<()> {
            // SAFETY: a zero-length vector is never dereferenced.
            check(unsafe {
                libc::syscall(libc::SYS_sendmmsg, -1, std::ptr::null_mut::<u8>(), 0, 0)
            })
        }

        pub(super) fn recvmsg(socket: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
            let mut byte = [0_u8; 1];
            let mut iov = libc::iovec {
                iov_base: byte.as_mut_ptr().cast(),
                iov_len: 1,
            };
            // SAFETY: all-zero is a valid empty `msghdr`; the one iovec points
            // at a live one-byte buffer.
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &raw mut iov;
            message.msg_iovlen = 1;
            // SAFETY: `message` describes valid, writable memory.
            check(unsafe { libc::recvmsg(socket.as_raw_fd(), &raw mut message, 0) } as libc::c_long)
        }

        pub(super) fn descriptor_is_open(fd: libc::c_int) -> std::io::Result<()> {
            // SAFETY: `F_GETFD` only reads descriptor flags.
            check(libc::c_long::from(unsafe {
                libc::fcntl(fd, libc::F_GETFD)
            }))
        }

        /// Let `fd` survive exec, as a descriptor Struo inherited without
        /// `O_CLOEXEC` would.
        pub(super) fn keep_across_exec(fd: libc::c_int) {
            // SAFETY: `F_SETFD` only changes descriptor flags.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
        }

        #[cfg(target_arch = "x86_64")]
        pub(super) fn x32_socket() -> std::io::Result<()> {
            // SAFETY: `socket` takes plain integers.
            check(unsafe {
                libc::syscall(
                    0x4000_0000 | libc::SYS_socket,
                    libc::AF_INET,
                    libc::SOCK_STREAM,
                    0,
                )
            })
        }

        #[cfg(not(target_arch = "x86_64"))]
        pub(super) fn x32_socket() -> std::io::Result<()> {
            Ok(())
        }
    }

    async fn run_filtered(mode: &str) -> std::process::Output {
        let mut command = child_command(mode);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::super::arm(&mut command).expect("this host can deny the network");
        command.output().await.unwrap()
    }

    fn child_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "network::tests::live::child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_MODE, mode);
        command
    }

    #[tokio::test]
    async fn inet_is_denied_while_unix_ipc_keeps_working() {
        let output = run_filtered("probe").await;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let eacces = format!("errno:{}", libc::EACCES);
        let enosys = format!("errno:{}", libc::ENOSYS);
        for (probe, expected) in [
            ("tcp4", eacces.as_str()),
            ("tcp6", eacces.as_str()),
            ("udp4", eacces.as_str()),
            ("connect4", eacces.as_str()),
            ("netlink", eacces.as_str()),
            ("sendmmsg", eacces.as_str()),
            ("io_uring_setup", enosys.as_str()),
            ("recvmsg", eacces.as_str()),
            ("unix_stream", "ok"),
            ("unix_pair", "ok"),
            ("unix_datagram_pair", "ok"),
        ] {
            assert!(
                stdout
                    .lines()
                    .any(|line| line == format!("{probe}={expected}")),
                "{probe} should be {expected}; stdout: {stdout}; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[tokio::test]
    async fn x32_syscalls_kill_the_process() {
        use std::os::unix::process::ExitStatusExt;

        let output = run_filtered("x32").await;
        assert_eq!(
            output.status.signal(),
            Some(libc::SIGSYS),
            "stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("x32_survived"));
    }

    #[tokio::test]
    async fn sockets_inherited_without_cloexec_do_not_reach_the_action() {
        use std::os::fd::AsRawFd;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let fd = listener.as_raw_fd();
        raw::keep_across_exec(fd);

        let mut unfiltered = child_command("inherited");
        unfiltered.env(INHERITED_FD, fd.to_string());
        let control = unfiltered.output().await.unwrap();
        let control = String::from_utf8_lossy(&control.stdout);
        assert!(
            control.lines().any(|line| line == "inherited=ok"),
            "the control run did not inherit the socket: {control}"
        );

        let mut filtered = child_command("inherited");
        filtered.env(INHERITED_FD, fd.to_string());
        super::super::arm(&mut filtered).expect("this host can deny the network");
        let output = filtered.output().await.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected = format!("inherited=errno:{}", libc::EBADF);
        assert!(
            stdout.lines().any(|line| line == expected),
            "{stdout}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn an_inet_socket_as_a_standard_stream_fails_the_spawn() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut command = child_command("noop");
        command
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(stream)))
            .stdout(Stdio::piped());
        super::super::arm(&mut command).expect("this host can deny the network");
        let error = command
            .spawn()
            .expect_err("an INET stdin reached the action");
        assert_eq!(error.raw_os_error(), Some(libc::EAFNOSUPPORT));
    }

    #[tokio::test]
    async fn a_unix_socket_as_a_standard_stream_is_allowed() {
        let (stdin, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut command = child_command("noop");
        command
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(stdin)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::super::arm(&mut command).expect("this host can deny the network");
        let output = command.output().await.unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("started=ok"),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
