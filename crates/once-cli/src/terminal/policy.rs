use crate::cli::{Format, Output, TerminalControls};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Policy {
    pub(crate) controls: bool,
    pub(crate) panel: bool,
}

impl Policy {
    pub(crate) fn new(output: Output) -> Self {
        let policy = Self::resolve(
            output,
            console::user_attended_stderr(),
            std::env::var("TERM").ok().as_deref(),
            once_events_client::environment::is_ci(),
        );
        if policy.controls {
            policy.with_vt(vt_ready())
        } else {
            policy
        }
    }

    fn resolve(output: Output, tty: bool, term: Option<&str>, ci: bool) -> Self {
        let interactive = output.format == Format::Human && tty && term != Some("dumb") && !ci;
        Self {
            controls: interactive && output.terminal_controls != TerminalControls::Never,
            panel: interactive && !output.quiet,
        }
    }

    fn with_vt(mut self, ready: bool) -> Self {
        self.controls &= ready;
        self
    }

    pub(crate) fn dashboard(self, url: &str) -> String {
        if self.controls && safe_url(url) {
            format!("\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\")
        } else {
            url.to_string()
        }
    }
}

#[cfg(windows)]
fn vt_ready() -> bool {
    anstyle_query::windows::enable_virtual_terminal_processing().is_ok()
}

#[cfg(not(windows))]
fn vt_ready() -> bool {
    true
}

fn safe_url(url: &str) -> bool {
    if !url.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return false;
    }
    let Ok(uri) = url.parse::<tonic::transport::Uri>() else {
        return false;
    };
    uri.host().is_some_and(|host| {
        uri.scheme_str() == Some("https")
            || (uri.scheme_str() == Some("http")
                && matches!(host, "localhost" | "127.0.0.1" | "[::1]"))
    }) && uri
        .authority()
        .is_some_and(|authority| !authority.as_str().contains('@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_is_independent_of_color_and_quiet() {
        let output = Output::new(Format::Human, true).with_color(crate::cli::ColorChoice::Never);
        let policy = Policy::resolve(output, true, None, false);
        assert!(policy.controls);
        assert!(!policy.panel);
        for (tty, term, ci) in [
            (false, None, false),
            (true, Some("dumb"), false),
            (true, None, true),
        ] {
            assert!(!Policy::resolve(output, tty, term, ci).controls);
        }
        assert!(!Policy::resolve(Output::new(Format::Json, false), true, None, false).controls);
        assert!(
            !Policy::resolve(
                output.with_terminal_controls(TerminalControls::Never),
                true,
                None,
                false
            )
            .controls
        );
    }

    #[test]
    fn vt_failure_disables_protocols_without_changing_panel_policy() {
        let policy = Policy {
            controls: true,
            panel: true,
        }
        .with_vt(false);
        assert!(!policy.controls);
        assert!(policy.panel);
    }

    #[test]
    fn hyperlinks_keep_the_url_visible_and_have_zero_extra_width() {
        let policy = Policy {
            controls: true,
            panel: true,
        };
        let url = "https://example.com/runs/1";
        let linked = policy.dashboard(url);
        assert_eq!(linked, format!("\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\"));
        assert_eq!(console::measure_text_width(&linked), url.len());
        for unsafe_url in [
            "https://example.com/\x1b",
            "https://example.com/\x07",
            "https://example.com/\u{009c}",
            "https://example.com/é",
            "https://user:secret@example.com",
            "file:///tmp/x",
            "http://example.com",
            "https://example.com/a b",
        ] {
            assert_eq!(policy.dashboard(unsafe_url), unsafe_url);
        }
        assert!(safe_url("http://localhost:4000/runs/1"));
    }
}
