#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use once_core::ResourceLimits;
use rustix::process::{getrlimit, setrlimit, Resource};

#[test]
fn default_memory_preserves_file_limits() {
    const CHILD: &str = "ONCE_TEST_FILE_LIMIT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "default_memory_preserves_file_limits",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        println!("{}", String::from_utf8_lossy(&output.stdout));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let mut before = getrlimit(Resource::Nofile);
    before.current = Some(1024.min(before.maximum.unwrap_or(u64::MAX) / 2));
    assert!(before.current.unwrap() > 0 && before.current != before.maximum);
    setrlimit(Resource::Nofile, before).unwrap();
    for _ in 0..4 {
        let limits = ResourceLimits::default();
        assert!(limits.memory_bytes > 0);
        let after = getrlimit(Resource::Nofile);
        println!("NOFILE before={before:?} after={after:?}");
        assert_eq!(after, before);
    }
}

fn unescape_mount(value: &str) -> PathBuf {
    PathBuf::from(
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

fn memory_max(path: &Path) -> Option<u64> {
    let value = match fs::read_to_string(path.join("memory.max")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("cannot read {}: {error}", path.display()),
    };
    match value.trim() {
        "max" => None,
        value => Some(value.parse().expect("numeric memory.max")),
    }
}

fn kernel_capacity() -> u64 {
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap();
    let host = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024;
    let membership = fs::read_to_string("/proc/self/cgroup").unwrap();
    let membership = Path::new(
        membership
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .expect("requires unified cgroup v2 membership"),
    );
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap();
    let (root, mount) = mountinfo
        .lines()
        .find_map(|line| {
            let (fields, filesystem) = line.split_once(" - ")?;
            if filesystem.split_whitespace().next()? != "cgroup2" {
                return None;
            }
            let fields: Vec<_> = fields.split_whitespace().collect();
            let root = unescape_mount(fields[3]);
            let mount = unescape_mount(fields[4]);
            membership.strip_prefix(&root).ok()?;
            Some((root, mount))
        })
        .expect("requires a visible cgroup v2 mount containing membership");
    let leaf = mount.join(membership.strip_prefix(root).unwrap());
    assert_ne!(leaf, mount, "requires nested membership");
    let root_capacity = memory_max(&mount).unwrap_or(host).min(host);
    let mut capacity = host;
    for ancestor in leaf.ancestors() {
        // The cgroup v2 mount root may have no memory controller files.
        if ancestor != mount {
            assert!(ancestor.join("memory.max").is_file());
        }
        if let Some(limit) = memory_max(ancestor) {
            assert!(limit > 0, "requires positive limits");
            capacity = capacity.min(limit);
        }
        if ancestor == mount {
            break;
        }
    }
    assert!(
        capacity < host && capacity < root_capacity,
        "requires a finite nested limit below host and visible mount-root capacity"
    );
    println!(
        "kernel host={host} root={root_capacity} process={capacity} membership={}",
        membership.display()
    );
    capacity
}

#[test]
#[ignore = "requires readable nested cgroup v2 memory limits below host and mount-root capacity"]
fn default_memory_uses_process_cgroup() {
    const CHILD: &str = "ONCE_TEST_MEMORY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "default_memory_uses_process_cgroup",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        println!("{}", String::from_utf8_lossy(&output.stdout));
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    if let Some(cgroup) = std::env::var_os("ONCE_TEST_MEMORY_CGROUP") {
        fs::write(
            Path::new(&cgroup).join("cgroup.procs"),
            std::process::id().to_string(),
        )
        .expect("cannot move isolated child into requested cgroup");
    }
    let capacity = kernel_capacity();
    let expected = capacity.saturating_mul(2) / 3;
    let limits = ResourceLimits::default();
    println!(
        "expected budget={expected} actual budget={}",
        limits.memory_bytes
    );
    assert_eq!(limits.memory_bytes, expected);
    assert!(limits.default_action_memory_bytes > 0);
    assert!(limits.default_action_memory_bytes <= expected);

    let override_bytes = capacity + 1024;
    assert_eq!(
        limits
            .clone()
            .with_memory_bytes(override_bytes)
            .memory_bytes,
        override_bytes
    );
    let disabled = limits.with_memory_bytes(0);
    assert_eq!(disabled.memory_bytes, 0);
    assert_eq!(disabled.default_action_memory_bytes, 0);
    assert_eq!(disabled.max_parallel_actions(), disabled.cpu_slots);
}
