use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nix::pty::{openpty, Winsize};

pub struct PtyProcess {
    pub child: Child,
    output: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
    group_active: bool,
}

impl PtyProcess {
    pub fn spawn(mut command: Command) -> Self {
        let pty = openpty(
            Some(&Winsize {
                ws_row: 30,
                ws_col: 100,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        let mut master = std::fs::File::from(pty.master);
        command.stderr(Stdio::from(pty.slave)).process_group(0);
        let child = command.spawn().unwrap();
        drop(command);
        let output = Arc::new(Mutex::new(String::new()));
        let captured = output.clone();
        let reader = std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match master.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(read) => {
                        let mut output = captured.lock().unwrap();
                        assert!(
                            output.len() < 2 * 1024 * 1024,
                            "test terminal output exceeded limit"
                        );
                        output.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    }
                }
            }
        });
        Self {
            child,
            output,
            reader: Some(reader),
            group_active: true,
        }
    }

    pub fn output(&self) -> String {
        self.output.lock().unwrap().clone()
    }

    pub fn finish(mut self) -> (ExitStatus, String) {
        let started = Instant::now();
        let pid = rustix::process::Pid::from_child(&self.child);
        let flags = rustix::process::WaitIdOptions::EXITED
            | rustix::process::WaitIdOptions::NOHANG
            | rustix::process::WaitIdOptions::NOWAIT;
        loop {
            if rustix::process::waitid(rustix::process::WaitId::Pid(pid), flags)
                .unwrap()
                .is_some()
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "once did not exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        self.kill_group();
        let status = self.child.wait().unwrap();
        while !self.reader.as_ref().unwrap().is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "terminal reader did not finish"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        self.reader.take().unwrap().join().unwrap();
        (status, self.output())
    }

    fn kill_group(&mut self) {
        if !std::mem::take(&mut self.group_active) {
            return;
        }
        let group = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).unwrap());
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        self.kill_group();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn command(workspace: &std::path::Path, home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_once"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("TERM", "xterm-256color")
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

pub fn graph_workspace(workspace: &std::path::Path, sleep: bool) {
    std::fs::create_dir_all(workspace.join("modules")).unwrap();
    let implementation = if sleep {
        "run_action(argv = [host_which(\"sleep\"), \"60\"], outputs = [], identifier = \"wait\")"
    } else {
        "write_path(out, \"built\")"
    };
    std::fs::write(workspace.join("modules/task.star"), format!(r#"
def _impl(ctx):
    out = declare_output("result.txt")
    {implementation}
    return {{"out": out}}

task = target_kind(
    providers = ["task_output"],
    capabilities = [capability("build", ["default"]), capability("run", ["default"]), capability("test", ["default"])],
    impl = _impl,
)
"#)).unwrap();
    let path = workspace.join("once.toml");
    writeln!(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "\n[modules]\npaths = [\"modules/*.star\"]\n\n[[target]]\nname = \"Task\"\nkind = \"task\""
    )
    .unwrap();
}
