use std::io::Write;
use std::sync::{Arc, Mutex, Weak};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::{Policy, StderrWriter};

static ACTIVE: Mutex<Weak<Status<StderrWriter>>> = Mutex::new(Weak::new());

struct State<W> {
    writer: W,
    finished: bool,
}

struct Status<W> {
    state: Mutex<State<W>>,
    label: String,
}

impl<W: Write> Status<W> {
    fn report(&self, state: &str, final_state: bool) {
        let Ok(mut inner) = self.state.lock() else {
            return;
        };
        self.write(&mut inner, state, final_state);
    }

    fn fail(&self) {
        if let Ok(mut inner) = self.state.try_lock() {
            self.write(&mut inner, "error", true);
        }
    }

    fn write(&self, inner: &mut State<W>, state: &str, final_state: bool) {
        if inner.finished {
            return;
        }
        inner.finished = final_state;
        let message = format!("{}: {state}", self.label);
        let _ = inner.writer.write_all(&encode(state, &message));
        let _ = inner.writer.flush();
    }
}

pub(crate) struct StatusGuard(Option<Arc<Status<StderrWriter>>>);

impl StatusGuard {
    pub(crate) fn start(policy: Policy, label: &str) -> Self {
        if !policy.controls {
            return Self(None);
        }
        let status = Arc::new(Status {
            state: Mutex::new(State {
                writer: StderrWriter,
                finished: false,
            }),
            label: sanitize(label, 256),
        });
        status.report("working", false);
        *ACTIVE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&status);
        Self(Some(status))
    }

    pub(crate) fn finish(&self, success: bool) {
        if let Some(status) = &self.0 {
            status.report(if success { "done" } else { "error" }, true);
        }
    }
}

impl Drop for StatusGuard {
    fn drop(&mut self) {
        if let Some(status) = &self.0 {
            if let Ok(mut state) = status.state.try_lock() {
                status.write(&mut state, "error", true);
            }
        }
    }
}

pub(crate) fn cancel_active() {
    let status = ACTIVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .upgrade();
    if let Some(status) = status {
        status.report("idle", true);
    }
}

pub(crate) fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(active) = ACTIVE.try_lock() {
            if let Some(status) = active.upgrade() {
                status.fail();
            }
        }
        previous(info);
    }));
}

fn encode(state: &str, message: &str) -> Vec<u8> {
    let message = STANDARD.encode(sanitize(message, 2048));
    format!("\x1b]7501;state={state}:app=once:msg={message}\x1b\\").into_bytes()
}

fn sanitize(text: &str, limit: usize) -> String {
    let mut output = String::new();
    for c in text.chars().filter(|c| {
        !c.is_control() && !matches!(c,
            '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
    }) {
        if output.len() + c.len_utf8() > limit { break; }
        output.push(c);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_bounded_sanitized_and_repeats_app() {
        for state in ["working", "done", "error", "idle"] {
            let report = encode(state, "a\n\x1b\u{009c}\u{202e}é");
            assert_eq!(
                String::from_utf8(report).unwrap(),
                format!(
                    "\x1b]7501;state={state}:app=once:msg={}\x1b\\",
                    STANDARD.encode("aé")
                )
            );
        }
        assert!(encode("working", &"é".repeat(5000)).len() < 4096);
        assert_eq!(sanitize("éé", 3), "é");
    }

    #[test]
    fn final_state_prevents_late_working_and_duplicate_completion() {
        let status = Status {
            state: Mutex::new(State {
                writer: Vec::new(),
                finished: false,
            }),
            label: "build".into(),
        };
        status.report("working", false);
        status.report("idle", true);
        status.report("working", false);
        status.report("error", true);
        let output = String::from_utf8(status.state.lock().unwrap().writer.clone()).unwrap();
        assert_eq!(output.matches("7501;").count(), 2);
        assert!(output.contains("state=idle"));
        assert!(!output.contains("state=error"));
    }

    #[test]
    fn concurrent_completion_reports_exactly_one_final_state() {
        let status = Arc::new(Status {
            state: Mutex::new(State {
                writer: Vec::new(),
                finished: false,
            }),
            label: "test".into(),
        });
        std::thread::scope(|scope| {
            for state in ["done", "idle", "error"] {
                let status = status.clone();
                scope.spawn(move || status.report(state, true));
            }
        });
        let output = status.state.lock().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.writer)
                .matches("7501;")
                .count(),
            1
        );
    }
}
