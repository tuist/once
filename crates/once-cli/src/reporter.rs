//! Beautiful terminal renderer for build and test runs.
//!
//! Subscribes to the [`RunEventBus`] and draws a layered dashboard on
//! stderr: completed targets scroll upward as short one-line summaries,
//! and a sticky panel at the bottom shows the summary line plus one
//! spinner per in-flight target with its current phase.
//!
//! On a non-TTY stderr (CI, piped, agent), the sticky panel is hidden
//! and only the per-completion lines are emitted, one per line, so the
//! output is grep-friendly and doesn't fight with the harness. Color
//! honors `--color=auto|always|never` and the standard `NO_COLOR`,
//! `CLICOLOR_FORCE`, and `TERM=dumb` environment variables via `console`.
//!
//! The reporter never blocks a producer: it consumes events on a
//! `broadcast` receiver and drops with a warning when it falls behind
//! the bus's ring. Producers only ever `send`.
//!
//! Verbosity has three levels that affect what captured child output
//! surfaces:
//!
//! - `Normal` prints captured stderr only for failed targets.
//! - `Verbose` also prints a short tail of captured output for every
//!   completed target.
//! - `ExtraVerbose` prints the full captured stdout+stderr for every
//!   target, prefixed by target id.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use console::{style, Style};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use once_core::{LogStream, Phase, RunEvent, RunEventBus, TargetResult};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

/// How color should be applied to the reporter's output.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    /// Colored output when stderr is a TTY and no override forbids it.
    #[default]
    Auto,
    /// Force colored output.
    Always,
    /// Suppress all color escape sequences.
    Never,
}

/// How much captured child output to surface for successful targets.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Verbosity {
    /// Only print captured output for failed targets. The default.
    #[default]
    Normal,
    /// Also print the last few lines of captured output for every target.
    Verbose,
    /// Print all captured output for every target, live-prefixed.
    ExtraVerbose,
}

/// Options controlling how the reporter renders a run.
#[derive(Clone, Debug)]
pub struct ReporterOptions {
    /// A short label naming the run, e.g. `"build ripgrep-cli"`.
    pub command_label: String,
    /// Status shown until the first target starts executing.
    pub initial_status: Option<String>,
    pub color: ColorMode,
    pub verbosity: Verbosity,
    /// When true, do not draw the sticky bottom panel; only render
    /// per-completion lines and the final summary. Set by `--quiet`.
    pub suppress_panel: bool,
}

/// A running terminal reporter subscribed to a [`RunEventBus`].
///
/// Drop or [`finish`](Self::finish) to tear down cleanly.
pub struct TerminalReporter {
    handle: JoinHandle<()>,
    multi: Arc<MultiProgress>,
}

impl TerminalReporter {
    /// Subscribe to the bus and spawn a background task that renders
    /// events as they arrive. Returns immediately.
    #[must_use]
    pub fn spawn(bus: &RunEventBus, options: ReporterOptions) -> Self {
        apply_color_mode(options.color);

        let draw_target = if options.suppress_panel {
            ProgressDrawTarget::hidden()
        } else {
            ProgressDrawTarget::stderr()
        };
        let multi = Arc::new(MultiProgress::with_draw_target(draw_target));

        let mut receiver = bus.subscribe();
        let render_multi = Arc::clone(&multi);
        let mut state = RenderState::new(render_multi, options);
        state.render_header();
        let handle = tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if state.handle_event(event) {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
                    Err(RecvError::Lagged(missed)) => {
                        state.warn_lagged(missed);
                    }
                }
            }
            state.finalize();
        });

        Self { handle, multi }
    }

    /// Wait for the reporter to finish rendering. This does not on its
    /// own tell the reporter to stop; it returns when the bus closes
    /// (all `RunEventBus` clones dropped) or a `RunCompleted` event has
    /// been consumed.
    pub async fn finish(self) {
        let _ = self.handle.await;
        let _ = self.multi.clear();
    }
}

fn apply_color_mode(mode: ColorMode) {
    match mode {
        ColorMode::Auto => {}
        ColorMode::Always => {
            console::set_colors_enabled_stderr(true);
            console::set_colors_enabled(true);
        }
        ColorMode::Never => {
            console::set_colors_enabled_stderr(false);
            console::set_colors_enabled(false);
        }
    }
}

struct RenderState {
    multi: Arc<MultiProgress>,
    options: ReporterOptions,
    started_at: Instant,
    summary: Option<ProgressBar>,
    status: Option<String>,
    active: HashMap<String, ActiveTarget>,
    captured: HashMap<String, CapturedOutput>,
    totals: Totals,
}

struct ActiveTarget {
    bar: ProgressBar,
    started_at: Instant,
    phase: Phase,
}

#[derive(Default)]
struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Function pointer that wraps a string in one of the reporter's
/// styling roles (green success, red failure, and so on). Aliased here
/// to keep the completion match arm readable and satisfy
/// clippy's `type_complexity` lint.
type StyleFn = fn(String) -> console::StyledObject<String>;

#[derive(Default, Clone, Copy)]
struct Totals {
    started: usize,
    cached: usize,
    built: usize,
    failed: usize,
    skipped: usize,
}

impl RenderState {
    fn new(multi: Arc<MultiProgress>, options: ReporterOptions) -> Self {
        let status = options.initial_status.clone();
        Self {
            multi,
            options,
            started_at: Instant::now(),
            summary: None,
            status,
            active: HashMap::new(),
            captured: HashMap::new(),
            totals: Totals::default(),
        }
    }

    fn render_header(&mut self) {
        let label = &self.options.command_label;
        let header = style(format!("once  {label}")).bold();
        let divider = style("─".repeat(40)).dim();
        self.emit(format!("\n  {header}\n  {divider}"));

        if !self.options.suppress_panel {
            let summary = self.multi.add(ProgressBar::new_spinner());
            summary.set_style(spinner_style_bold());
            summary.set_message(self.summary_message());
            summary.enable_steady_tick(Duration::from_millis(80));
            self.summary = Some(summary);
        }
    }

    /// Print a stand-alone line. When the sticky panel is active
    /// (`MultiProgress` on a TTY), route through `println` so it lands
    /// above the bars. Otherwise write straight to stderr so lines
    /// still appear when indicatif's draw target is hidden.
    fn emit(&self, text: impl AsRef<str>) {
        use std::io::Write as _;
        if console::user_attended_stderr() {
            let _ = self.multi.println(text.as_ref());
            return;
        }
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        let _ = handle.write_all(text.as_ref().as_bytes());
        let _ = handle.write_all(b"\n");
    }

    /// Returns `true` when the run is complete and the reporter can stop.
    ///
    /// The wildcard arm covers not just future `RunEvent` variants
    /// (the enum is `#[non_exhaustive]`) but also every variant that
    /// today is a no-op for the reporter: `RunStarted`, `TargetQueued`,
    /// and the test-suite/test-case events. Consolidating them keeps
    /// clippy's `match_same_arms` happy without a target-by-target
    /// list of `=> false` branches.
    fn handle_event(&mut self, event: RunEvent) -> bool {
        match event {
            RunEvent::TargetStarted { target_id, .. } => {
                self.status = None;
                self.totals.started += 1;
                self.on_target_started(&target_id);
                self.refresh_summary();
                false
            }
            RunEvent::TargetPhase {
                target_id, phase, ..
            } => {
                self.on_target_phase(&target_id, phase);
                false
            }
            RunEvent::LogChunk {
                target_id,
                stream,
                bytes,
                ..
            } => {
                self.on_log_chunk(&target_id, stream, &bytes);
                false
            }
            RunEvent::TargetCompleted {
                target_id,
                result,
                was_cached,
                duration_ms,
                ..
            } => {
                self.on_target_completed(&target_id, result, was_cached, duration_ms);
                self.refresh_summary();
                false
            }
            RunEvent::RunCompleted { .. } => true,
            _ => false,
        }
    }

    fn on_target_started(&mut self, target_id: &str) {
        if self.options.suppress_panel {
            return;
        }
        if self.active.contains_key(target_id) {
            return;
        }
        let bar = self.multi.add(ProgressBar::new_spinner());
        bar.set_style(spinner_style_child());
        bar.set_message(active_line(target_id, Phase::Executing, Duration::ZERO));
        bar.enable_steady_tick(Duration::from_millis(80));
        let entry = ActiveTarget {
            bar,
            started_at: Instant::now(),
            phase: Phase::Executing,
        };
        self.active.insert(target_id.to_string(), entry);
    }

    fn on_target_phase(&mut self, target_id: &str, phase: Phase) {
        if let Some(entry) = self.active.get_mut(target_id) {
            entry.phase = phase;
            let elapsed = entry.started_at.elapsed();
            entry
                .bar
                .set_message(active_line(target_id, phase, elapsed));
        }
    }

    fn on_log_chunk(&mut self, target_id: &str, stream: LogStream, bytes: &[u8]) {
        let entry = self.captured.entry(target_id.to_string()).or_default();
        let buf = match stream {
            LogStream::Stdout => &mut entry.stdout,
            LogStream::Stderr => &mut entry.stderr,
        };
        buf.extend_from_slice(bytes);

        if matches!(self.options.verbosity, Verbosity::ExtraVerbose) {
            for line in split_lines(bytes) {
                let prefix = style(format!("{target_id}: ")).dim();
                let text = String::from_utf8_lossy(line);
                let styled = match stream {
                    LogStream::Stdout => style(text.to_string()),
                    LogStream::Stderr => style(text.to_string()).yellow(),
                };
                self.emit(format!("  {prefix}{styled}"));
            }
        }
    }

    fn on_target_completed(
        &mut self,
        target_id: &str,
        result: TargetResult,
        was_cached: bool,
        duration_ms: i64,
    ) {
        if let Some(entry) = self.active.remove(target_id) {
            entry.bar.finish_and_clear();
        }

        let duration = format_duration_ms(duration_ms);
        let (icon, tag, style_fn): (&str, &str, StyleFn) = match (result, was_cached) {
            (TargetResult::Succeeded, true) => ("✓", "cached", |s| style(s).green()),
            (TargetResult::Succeeded, false) => ("✓", "built", |s| style(s).green()),
            (TargetResult::Failed, _) => ("✗", "failed", |s| style(s).red().bold()),
            (TargetResult::Skipped, _) => ("○", "skipped", |s| style(s).dim()),
            (TargetResult::Cancelled, _) => ("⊘", "cancelled", |s| style(s).yellow()),
        };

        match (result, was_cached) {
            (TargetResult::Succeeded, true) => self.totals.cached += 1,
            (TargetResult::Succeeded, false) => self.totals.built += 1,
            (TargetResult::Failed | TargetResult::Cancelled, _) => self.totals.failed += 1,
            (TargetResult::Skipped, _) => self.totals.skipped += 1,
        }

        let icon = style_fn(icon.to_string());
        let name = style(target_id.to_string()).bold();
        let name = pad_right(&name.to_string(), 32);
        let tag_col = style_fn(pad_right(tag, 10));
        let dur = style(duration).dim();
        self.emit(format!("  {icon} {name} {tag_col} {dur}"));

        let captured = self.captured.remove(target_id).unwrap_or_default();
        if matches!(result, TargetResult::Failed) {
            self.print_captured(target_id, &captured, /*failure=*/ true);
        } else if matches!(self.options.verbosity, Verbosity::Verbose) {
            self.print_captured_tail(&captured);
        } else if matches!(self.options.verbosity, Verbosity::ExtraVerbose) {
            // Already streamed live via on_log_chunk.
        }
    }

    fn print_captured(&self, _target_id: &str, captured: &CapturedOutput, failure: bool) {
        let mut lines: Vec<Vec<u8>> = Vec::new();
        for line in split_lines(&captured.stderr) {
            lines.push(line.to_vec());
        }
        if lines.is_empty() {
            for line in split_lines(&captured.stdout) {
                lines.push(line.to_vec());
            }
        }
        if lines.is_empty() {
            return;
        }
        let bar_style: Box<dyn Fn(String) -> console::StyledObject<String>> = if failure {
            Box::new(|s| style(s).red())
        } else {
            Box::new(|s| style(s).dim())
        };
        self.emit("");
        for line in lines {
            let text = String::from_utf8_lossy(&line);
            let prefix = bar_style("│".to_string());
            self.emit(format!("    {prefix} {text}"));
        }
        self.emit("");
    }

    fn print_captured_tail(&self, captured: &CapturedOutput) {
        const TAIL_LINES: usize = 3;
        let mut all: Vec<Vec<u8>> = split_lines(&captured.stdout)
            .into_iter()
            .chain(split_lines(&captured.stderr))
            .map(<[u8]>::to_vec)
            .collect();
        if all.is_empty() {
            return;
        }
        let start = all.len().saturating_sub(TAIL_LINES);
        for line in all.drain(start..) {
            let text = String::from_utf8_lossy(&line);
            let prefix = style("│".to_string()).dim();
            self.emit(format!("    {prefix} {text}"));
        }
    }

    fn refresh_summary(&mut self) {
        if let Some(summary) = &self.summary {
            summary.set_message(self.summary_message());
        }
    }

    fn summary_message(&self) -> String {
        let label = &self.options.command_label;
        if let Some(status) = &self.status {
            return format!("{} · {}", style(label).bold(), style(status).dim());
        }
        let running = self.active.len();
        let done =
            self.totals.built + self.totals.cached + self.totals.failed + self.totals.skipped;
        let cache_pct = if self.totals.built + self.totals.cached > 0 {
            let denom = self.totals.built + self.totals.cached;
            (self.totals.cached * 100) / denom
        } else {
            0
        };
        let stats = style(format!(
            "{done} done · {running} running · cache {cache_pct}%"
        ))
        .dim();
        format!("{} · {stats}", style(label).bold())
    }

    fn warn_lagged(&self, missed: u64) {
        let msg = style(format!("(reporter fell behind; {missed} events dropped)"))
            .yellow()
            .dim();
        self.emit(format!("  {msg}"));
    }

    fn finalize(&mut self) {
        for (_, entry) in self.active.drain() {
            entry.bar.finish_and_clear();
        }
        if let Some(summary) = self.summary.take() {
            summary.finish_and_clear();
        }
        let elapsed = self.started_at.elapsed();
        let t = self.totals;
        let mut segments: Vec<console::StyledObject<String>> = Vec::new();
        if t.failed > 0 {
            segments.push(style(format!("{} failed", t.failed)).red().bold());
        }
        if t.built > 0 {
            segments.push(style(format!("{} built", t.built)).green());
        }
        if t.cached > 0 {
            segments.push(style(format!("{} cached", t.cached)).dim());
        }
        if t.skipped > 0 {
            segments.push(style(format!("{} skipped", t.skipped)).dim());
        }
        if segments.is_empty() {
            segments.push(style("no targets".to_string()).dim());
        }
        let joined = segments
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let done_word = if t.failed > 0 {
            style("Failed".to_string()).red().bold()
        } else {
            style("Done".to_string()).green().bold()
        };
        let dur = style(format!("in {}", format_duration(elapsed))).dim();
        self.emit(format!("\n  {done_word}  {joined}  {dur}\n"));
    }
}

fn spinner_style_bold() -> ProgressStyle {
    ProgressStyle::with_template("  {spinner:.cyan.bold} {msg}")
        .expect("valid spinner template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

fn spinner_style_child() -> ProgressStyle {
    ProgressStyle::with_template("    {spinner:.cyan} {msg}")
        .expect("valid spinner template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

fn active_line(target_id: &str, phase: Phase, elapsed: Duration) -> String {
    let name = pad_right(target_id, 32);
    let phase_label = phase_label(phase);
    let phase_col = pad_right(phase_label, 10);
    let phase_styled = phase_style(phase).apply_to(phase_col).to_string();
    let dur = style(format_duration(elapsed)).dim();
    format!("{name} {phase_styled} {dur}")
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Queued => "queued",
        Phase::CacheChecking => "cache-lookup",
        Phase::Preparing => "preparing",
        Phase::Executing => "executing",
        Phase::Capturing => "capturing",
        Phase::Publishing => "publishing",
    }
}

fn phase_style(phase: Phase) -> Style {
    match phase {
        Phase::Queued | Phase::CacheChecking | Phase::Preparing => Style::new().dim(),
        Phase::Executing => Style::new().cyan(),
        Phase::Capturing | Phase::Publishing => Style::new().magenta(),
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{}ms", duration.as_millis())
    } else if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = duration.as_secs() / 60;
        let s = duration.as_secs() % 60;
        format!("{mins}m{s:02}s")
    }
}

fn format_duration_ms(duration_ms: i64) -> String {
    if duration_ms < 0 {
        return "?".to_string();
    }
    let ms = u64::try_from(duration_ms).unwrap_or(0);
    format_duration(Duration::from_millis(ms))
}

fn pad_right(text: &str, width: usize) -> String {
    let display_width = console::measure_text_width(text);
    if display_width >= width {
        text.to_string()
    } else {
        let pad = width - display_width;
        format!("{text}{}", " ".repeat(pad))
    }
}

fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_switches_units() {
        assert_eq!(format_duration(Duration::from_millis(120)), "120ms");
        assert_eq!(format_duration(Duration::from_millis(1200)), "1.2s");
        assert_eq!(format_duration(Duration::from_secs(75)), "1m15s");
    }

    #[test]
    fn split_lines_drops_trailing_newline_and_empty_frames() {
        let lines = split_lines(b"a\nb\n\nc");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], b"a");
        assert_eq!(lines[1], b"b");
        assert_eq!(lines[2], b"c");
    }

    #[test]
    fn pad_right_measures_display_width() {
        let padded = pad_right("hi", 6);
        assert_eq!(console::measure_text_width(&padded), 6);
    }

    #[tokio::test]
    async fn reporter_finishes_when_run_completed_seen() {
        let bus = RunEventBus::new(8);
        let reporter = TerminalReporter::spawn(
            &bus,
            ReporterOptions {
                command_label: "build test".into(),
                initial_status: Some("loading graph".into()),
                color: ColorMode::Never,
                verbosity: Verbosity::Normal,
                suppress_panel: true,
            },
        );
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 0 });
        bus.publish(RunEvent::RunCompleted {
            at_epoch_ms: 1,
            exit_status: 0,
        });
        drop(bus);
        // finish awaits the render task and clears the multi progress.
        reporter.finish().await;
    }

    #[test]
    fn initial_status_is_replaced_when_a_target_starts() {
        let multi = Arc::new(MultiProgress::with_draw_target(ProgressDrawTarget::hidden()));
        let mut state = RenderState::new(
            multi,
            ReporterOptions {
                command_label: "build xcode".into(),
                initial_status: Some("loading graph".into()),
                color: ColorMode::Never,
                verbosity: Verbosity::Normal,
                suppress_panel: false,
            },
        );

        assert!(state.summary_message().contains("loading graph"));
        state.handle_event(RunEvent::TargetStarted {
            at_epoch_ms: 0,
            target_id: "xcode".into(),
        });
        assert!(!state.summary_message().contains("loading graph"));
        assert!(state.summary_message().contains("1 running"));
    }
}
