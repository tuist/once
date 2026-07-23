//! Markdown CLI reference generator.
//!
//! Walks the `clap` [`Command`] tree exposed by [`Cli`] and emits one
//! markdown file per leaf or intermediate command into `out`, plus a
//! top-level `index.md`. Driven by the hidden `once reference --out`
//! subcommand and called from the docs build (`npm run
//! build:reference`), so the website's flag, synopsis, and exit-code
//! sections always reflect the real clap definitions instead of
//! drifting from the code.

use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command, CommandFactory};

use crate::cli::Cli;

/// Generate the full reference under `out`.
///
/// Returns a successful exit code on completion. Layout produced:
///
/// ```text
/// out/
///   index.md            # link list of all commands
///   build.md
///   run.md
///   ...
///   cache.md            # `once cache` (lists its subs)
///   cache/
///     stats.md
///     get.md
///     ...
/// ```
pub fn generate(out: &Path) -> Result<ExitCode> {
    fs::create_dir_all(out)
        .with_context(|| format!("creating reference output dir `{}`", out.display()))?;

    let cli_dir = out.join("cli");
    fs::create_dir_all(&cli_dir)
        .with_context(|| format!("creating reference cli dir `{}`", cli_dir.display()))?;

    let mut root = Cli::command();
    // `build()` resolves clap's deferred defaults (help template,
    // global args inherited from the parent, etc.) so what we read
    // matches the help text users see at runtime.
    root.build();

    let mut entries: Vec<CommandEntry> = Vec::new();
    walk(&root, &mut Vec::new(), &mut entries, true);
    // Hidden commands and `help` would clutter the index; the walker
    // already skips both.

    let bin_name = root.get_name().to_string();
    write_command_files(&cli_dir, &bin_name, &entries)?;
    write_index(&cli_dir, &bin_name, &entries)?;

    let mcp_dir = out.join("mcp");
    fs::create_dir_all(&mcp_dir)
        .with_context(|| format!("creating reference mcp dir `{}`", mcp_dir.display()))?;
    write_mcp_tools_page(&mcp_dir)?;

    Ok(ExitCode::SUCCESS)
}

struct CommandEntry {
    /// Path segments after the binary name (e.g. `["cache", "stats"]`).
    /// Empty for the root command itself, which we render at
    /// `index.md` only; the per-command files start at depth 1.
    path: Vec<String>,
    about: Option<String>,
    long_about: Option<String>,
    /// Concrete options (not the global `--help` / `--version` clap
    /// adds), in clap declaration order.
    options: Vec<OptionEntry>,
    positionals: Vec<PositionalEntry>,
    /// Direct child subcommand path segments. Used by intermediate
    /// command pages to link to their children.
    children: Vec<String>,
}

struct OptionEntry {
    name: String,
    description: Option<String>,
    takes_value: bool,
    value_name: Option<String>,
    default: Option<String>,
}

struct PositionalEntry {
    name: String,
    description: Option<String>,
    required: bool,
}

fn walk(cmd: &Command, path: &mut Vec<String>, entries: &mut Vec<CommandEntry>, is_root: bool) {
    if !is_root {
        let children: Vec<String> = cmd
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
            .map(|sub| sub.get_name().to_string())
            .collect();
        entries.push(CommandEntry {
            path: path.clone(),
            about: cmd.get_about().map(ToString::to_string),
            long_about: cmd.get_long_about().map(ToString::to_string),
            options: collect_options(cmd),
            positionals: collect_positionals(cmd),
            children,
        });
    }
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        path.push(sub.get_name().to_string());
        walk(sub, path, entries, false);
        path.pop();
    }
}

fn collect_options(cmd: &Command) -> Vec<OptionEntry> {
    cmd.get_arguments()
        .filter(|arg| !arg.is_positional())
        .filter(|arg| !arg.is_hide_set())
        .filter(|arg| arg.get_id() != "help" && arg.get_id() != "version")
        .map(option_entry)
        .collect()
}

fn collect_positionals(cmd: &Command) -> Vec<PositionalEntry> {
    cmd.get_arguments()
        .filter(|arg| arg.is_positional())
        .filter(|arg| !arg.is_hide_set())
        .map(positional_entry)
        .collect()
}

fn option_entry(arg: &Arg) -> OptionEntry {
    let mut name = String::new();
    if let Some(short) = arg.get_short() {
        write!(&mut name, "-{short}").ok();
    }
    if let Some(long) = arg.get_long() {
        if !name.is_empty() {
            name.push_str(", ");
        }
        write!(&mut name, "--{long}").ok();
    }
    if name.is_empty() {
        name = arg.get_id().to_string();
    }
    let takes_value = !matches!(
        arg.get_action(),
        ArgAction::SetTrue
            | ArgAction::SetFalse
            | ArgAction::Count
            | ArgAction::Help
            | ArgAction::Version
    );
    let value_name = arg
        .get_value_names()
        .and_then(|names| names.first().map(ToString::to_string));
    let default = arg
        .get_default_values()
        .first()
        .map(|v| v.to_string_lossy().into_owned());
    OptionEntry {
        name,
        description: arg.get_help().map(ToString::to_string),
        takes_value,
        value_name,
        default,
    }
}

fn positional_entry(arg: &Arg) -> PositionalEntry {
    PositionalEntry {
        name: arg
            .get_value_names()
            .and_then(|names| names.first().map(ToString::to_string))
            .unwrap_or_else(|| arg.get_id().to_string()),
        description: arg.get_help().map(ToString::to_string),
        required: arg.is_required_set(),
    }
}

fn render_command_page(bin: &str, entry: &CommandEntry) -> String {
    let mut body = String::new();
    let command_path = format!("{bin} {}", entry.path.join(" "));
    writeln!(&mut body, "# `{command_path}`\n").ok();
    if let Some(about) = &entry.about {
        writeln!(&mut body, "{about}\n").ok();
    }

    let synopsis = build_synopsis(&command_path, entry);
    writeln!(&mut body, "## Synopsis\n").ok();
    writeln!(&mut body, "```text\n{synopsis}\n```\n").ok();

    if let Some(long) = entry
        .long_about
        .as_deref()
        .and_then(|l| trim_leading_about(l, entry.about.as_deref()))
    {
        writeln!(&mut body, "## Description\n").ok();
        writeln!(&mut body, "{long}\n").ok();
    }

    render_arguments(&mut body, &entry.positionals);
    render_options(&mut body, &entry.options);
    render_subcommands(&mut body, bin, &entry.path, &entry.children);

    body
}

/// Compose the synopsis line: command path, `[OPTIONS]` placeholder
/// when flags exist, then each positional in declaration order, then
/// a `<SUBCOMMAND>` placeholder if children exist.
fn build_synopsis(command_path: &str, entry: &CommandEntry) -> String {
    let mut synopsis = command_path.to_string();
    if !entry.options.is_empty() {
        synopsis.push_str(" [OPTIONS]");
    }
    for positional in &entry.positionals {
        if positional.required {
            write!(&mut synopsis, " <{}>", positional.name).ok();
        } else {
            write!(&mut synopsis, " [{}]", positional.name).ok();
        }
    }
    if !entry.children.is_empty() {
        synopsis.push_str(" <SUBCOMMAND>");
    }
    synopsis
}

fn render_arguments(body: &mut String, positionals: &[PositionalEntry]) {
    if positionals.is_empty() {
        return;
    }
    writeln!(body, "## Arguments\n").ok();
    writeln!(body, "| Argument | Required | Description |").ok();
    writeln!(body, "| --- | --- | --- |").ok();
    for positional in positionals {
        let required = if positional.required { "yes" } else { "no" };
        let description = positional.description.as_deref().unwrap_or("");
        let name = &positional.name;
        writeln!(body, "| `<{name}>` | {required} | {description} |").ok();
    }
    body.push('\n');
}

fn render_options(body: &mut String, options: &[OptionEntry]) {
    if options.is_empty() {
        return;
    }
    writeln!(body, "## Options\n").ok();
    writeln!(body, "| Flag | Value | Default | Description |").ok();
    writeln!(body, "| --- | --- | --- | --- |").ok();
    for opt in options {
        let value = if opt.takes_value {
            format!("`<{}>`", opt.value_name.as_deref().unwrap_or("VALUE"))
        } else {
            "(flag)".to_string()
        };
        let default = match opt.default.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => format!("`{d}`"),
            None => String::new(),
        };
        let description = opt.description.as_deref().unwrap_or("");
        let name = &opt.name;
        writeln!(body, "| `{name}` | {value} | {default} | {description} |").ok();
    }
    body.push('\n');
}

fn render_subcommands(body: &mut String, bin: &str, path: &[String], children: &[String]) {
    if children.is_empty() {
        return;
    }
    writeln!(body, "## Subcommands\n").ok();
    // Hoist `parent` to a local so every interpolation in the bullet
    // line is an inline capture; mixing implicit captures and
    // positional placeholders here is unnecessarily hard to read.
    let parent = path.join(" ");
    for child in children {
        let mut child_path = path.to_vec();
        child_path.push(child.clone());
        let child_link = format!("/reference/cli/{}", child_path.join("/"));
        writeln!(body, "- [`{bin} {parent} {child}`]({child_link})").ok();
    }
    body.push('\n');
}

fn write_command_files(out: &Path, bin: &str, entries: &[CommandEntry]) -> Result<()> {
    for entry in entries {
        let body = render_command_page(bin, entry);
        let path = file_path_for(out, &entry.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating reference subdir `{}`", parent.display()))?;
        }
        fs::write(&path, trim_trailing_blank_lines(body))
            .with_context(|| format!("writing reference page `{}`", path.display()))?;
    }
    Ok(())
}

fn trim_trailing_blank_lines(mut body: String) -> String {
    while body.ends_with("\n\n") {
        body.pop();
    }
    body
}

/// Return the prose portion of a clap `long_about`, dropping the
/// leading "about" sentence clap auto-prepends. Returns `None` when
/// there is no extra prose past the summary.
fn trim_leading_about<'a>(long: &'a str, about: Option<&str>) -> Option<&'a str> {
    let trimmed = long.trim_start();
    let trailing = match about {
        Some(about) => {
            // The leading summary may or may not end with a period; try
            // both and pick whichever actually matched. After stripping,
            // also peel off the blank line that separates the summary
            // from the prose so we don't render a hanging newline.
            let without_summary = trimmed
                .strip_prefix(about)
                .or_else(|| trimmed.strip_prefix(about.trim_end_matches('.')))
                .or_else(|| {
                    about
                        .strip_suffix('.')
                        .and_then(|stripped| trimmed.strip_prefix(stripped))
                });
            without_summary.map_or(trimmed, |rest| {
                rest.trim_start_matches(|c: char| c == '.' || c.is_whitespace())
            })
        }
        None => trimmed,
    };
    if trailing.is_empty() {
        None
    } else {
        Some(trailing)
    }
}

fn file_path_for(out: &Path, path: &[String]) -> PathBuf {
    // Top-level commands live as `<name>.md`; nested commands live
    // under `<parent>/<child>.md` so the URL maps cleanly to
    // `/reference/cli/<parent>/<child>`.
    let mut buf = out.to_path_buf();
    for (i, segment) in path.iter().enumerate() {
        if i + 1 == path.len() {
            buf.push(format!("{segment}.md"));
        } else {
            buf.push(segment);
        }
    }
    buf
}

/// Render `mcp/tools.md` from the shared catalog so the reference
/// page can't drift from the tools the server advertises in
/// `tools/list`. Stable prose (transport, handshake, Claude Desktop
/// wiring) stays hand-authored in `docs/reference/mcp/index.md`.
fn write_mcp_tools_page(out: &Path) -> Result<()> {
    let body = render_mcp_tools_page();
    let path = out.join("tools.md");
    fs::write(&path, body)
        .with_context(|| format!("writing reference mcp tools page `{}`", path.display()))?;
    Ok(())
}

fn render_mcp_tools_page() -> String {
    let mut body = String::new();
    writeln!(&mut body, "# Model Context Protocol Tools\n").ok();
    writeln!(
        &mut body,
        "Every tool the [`once mcp`](/reference/cli/mcp) [Model Context Protocol](https://modelcontextprotocol.io/) server advertises in `tools/list`, with its input schema and a worked return example.\n"
    )
    .ok();
    for tool in crate::commands::mcp::tool_catalog() {
        let name = tool.name;
        let description = tool.description;
        writeln!(&mut body, "## `{name}`\n").ok();
        writeln!(&mut body, "{description}\n").ok();
        writeln!(&mut body, "{}\n", tool.long_description).ok();
        let schema = serde_json::to_string_pretty(&tool.input_schema)
            .unwrap_or_else(|_| tool.input_schema.to_string());
        writeln!(&mut body, "**Input schema**\n").ok();
        writeln!(&mut body, "```json\n{schema}\n```\n").ok();
        writeln!(&mut body, "**Example return**\n").ok();
        writeln!(&mut body, "```json\n{}\n```\n", tool.example_return).ok();
    }
    format!("{}\n", body.trim_end())
}

fn write_index(out: &Path, bin: &str, entries: &[CommandEntry]) -> Result<()> {
    let mut body = String::new();
    writeln!(&mut body, "# Command-line Reference\n").ok();
    writeln!(
        &mut body,
        "Every subcommand the `{bin}` binary exposes, with its synopsis, options, and arguments. Use the sidebar to jump to a specific command.\n"
    )
    .ok();
    writeln!(&mut body, "## Commands\n").ok();
    let mut top_level: Vec<&CommandEntry> = entries.iter().filter(|e| e.path.len() == 1).collect();
    top_level.sort_by(|a, b| a.path.cmp(&b.path));
    for entry in top_level {
        let about = entry.about.as_deref().unwrap_or("");
        writeln!(
            &mut body,
            "- [`{bin} {name}`](/reference/cli/{name}): {about}",
            name = entry.path[0]
        )
        .ok();
    }
    let path = out.join("index.md");
    fs::write(&path, body)
        .with_context(|| format!("writing reference index `{}`", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ---------- trim_leading_about ----------

    #[test]
    fn trim_leading_about_returns_none_when_long_matches_about_exactly() {
        assert_eq!(
            trim_leading_about("Build a target.", Some("Build a target.")),
            None
        );
    }

    #[test]
    fn trim_leading_about_drops_the_summary_and_blank_separator() {
        let long = "Build a target.\n\nExtra prose explaining the verb.";
        assert_eq!(
            trim_leading_about(long, Some("Build a target.")),
            Some("Extra prose explaining the verb.")
        );
    }

    #[test]
    fn trim_leading_about_tolerates_about_without_trailing_period() {
        // clap's doc-comment derive strips the trailing period from
        // `about` even though `long_about` keeps it.
        let long = "Build a target.\n\nThe long form.";
        assert_eq!(
            trim_leading_about(long, Some("Build a target")),
            Some("The long form.")
        );
    }

    #[test]
    fn trim_leading_about_tolerates_long_without_trailing_period() {
        let long = "Build a target\n\nThe long form.";
        assert_eq!(
            trim_leading_about(long, Some("Build a target.")),
            Some("The long form.")
        );
    }

    #[test]
    fn trim_leading_about_keeps_the_whole_long_when_about_is_missing() {
        let long = "Standalone prose with no about.";
        assert_eq!(
            trim_leading_about(long, None),
            Some("Standalone prose with no about.")
        );
    }

    #[test]
    fn trim_leading_about_returns_none_for_empty_input() {
        assert_eq!(trim_leading_about("", None), None);
        assert_eq!(trim_leading_about("   \n  ", None), None);
    }

    // ---------- file_path_for ----------

    #[test]
    fn file_path_for_lays_top_level_commands_at_the_root() {
        let out = Path::new("docs/reference/cli");
        let got = file_path_for(out, &["build".to_string()]);
        assert_eq!(got, PathBuf::from("docs/reference/cli/build.md"));
    }

    #[test]
    fn file_path_for_nests_subcommands_under_their_parents() {
        let out = Path::new("docs/reference/cli");
        let got = file_path_for(out, &["cache".to_string(), "stats".to_string()]);
        assert_eq!(got, PathBuf::from("docs/reference/cli/cache/stats.md"));
    }

    #[test]
    fn file_path_for_handles_three_levels_of_nesting() {
        let out = Path::new("docs/reference/cli");
        let got = file_path_for(
            out,
            &["cache".to_string(), "action".to_string(), "get".to_string()],
        );
        assert_eq!(got, PathBuf::from("docs/reference/cli/cache/action/get.md"));
    }

    // ---------- walk ----------

    fn synthetic_root() -> Command {
        // Hand-built clap tree: one top-level visible command, one
        // hidden command (must be skipped), one parent with a leaf
        // child (must descend), and a final command with no children
        // so the walker exits cleanly.
        Command::new("once")
            .subcommand(
                Command::new("build")
                    .about("Build a target")
                    .long_about("Build a target.\n\nResolves and runs.")
                    .arg(Arg::new("target").required(true)),
            )
            .subcommand(Command::new("internal-secret").about("Hidden").hide(true))
            .subcommand(
                Command::new("cache")
                    .about("Cache management")
                    .subcommand(Command::new("stats").about("Print cache stats")),
            )
    }

    #[test]
    fn walk_skips_hidden_subcommands_and_descends_into_children() {
        let mut root = synthetic_root();
        root.build();
        let mut entries = Vec::new();
        walk(&root, &mut Vec::new(), &mut entries, true);
        let paths: Vec<Vec<String>> = entries.iter().map(|e| e.path.clone()).collect();
        // `internal-secret` must not appear; `cache` parent and its
        // `stats` child both do; `build` is a top-level entry.
        assert!(paths.contains(&vec!["build".to_string()]));
        assert!(paths.contains(&vec!["cache".to_string()]));
        assert!(paths.contains(&vec!["cache".to_string(), "stats".to_string()]));
        assert!(!paths
            .iter()
            .any(|p| p.contains(&"internal-secret".to_string())));
    }

    #[test]
    fn walk_records_about_and_long_about_when_present() {
        let mut root = synthetic_root();
        root.build();
        let mut entries = Vec::new();
        walk(&root, &mut Vec::new(), &mut entries, true);
        let build = entries
            .iter()
            .find(|e| e.path == ["build".to_string()])
            .expect("build entry");
        assert_eq!(build.about.as_deref(), Some("Build a target"));
        assert_eq!(
            build.long_about.as_deref(),
            Some("Build a target.\n\nResolves and runs.")
        );
        // The `cache` parent records its single visible child so the
        // page can link to it.
        let cache = entries
            .iter()
            .find(|e| e.path == ["cache".to_string()])
            .expect("cache entry");
        assert_eq!(cache.children, vec!["stats".to_string()]);
    }

    // ---------- generate (end-to-end) ----------

    #[test]
    fn write_command_files_renders_synopsis_options_arguments_and_subcommands() {
        let mut root = synthetic_root();
        root.build();
        let mut entries = Vec::new();
        walk(&root, &mut Vec::new(), &mut entries, true);
        let tmp = TempDir::new().unwrap();
        write_command_files(tmp.path(), "once", &entries).unwrap();

        let build = std::fs::read_to_string(tmp.path().join("build.md")).unwrap();
        assert!(build.contains("# `once build`"));
        assert!(build.contains("```text\nonce build <target>\n```"));
        assert!(build.contains("## Description"));
        // The about line still prints above the synopsis; only the
        // trailing prose appears under Description.
        assert!(build.contains("Resolves and runs."));
        assert!(!build.contains("Build a target.\n\nResolves and runs."));

        let cache = std::fs::read_to_string(tmp.path().join("cache.md")).unwrap();
        assert!(cache.contains("## Subcommands"));
        // Each child gets its own labelled link, and the format-string
        // cleanup keeps this in lockstep with the iteration variable.
        assert!(cache.contains("- [`once cache stats`](/reference/cli/cache/stats)"));

        let stats = std::fs::read_to_string(tmp.path().join("cache/stats.md")).unwrap();
        assert!(stats.contains("# `once cache stats`"));
    }

    #[test]
    fn write_index_lists_top_level_commands_in_sorted_order() {
        let mut root = synthetic_root();
        root.build();
        let mut entries = Vec::new();
        walk(&root, &mut Vec::new(), &mut entries, true);
        let tmp = TempDir::new().unwrap();
        write_index(tmp.path(), "once", &entries).unwrap();
        let index = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
        // No "Generated from … crates/once-cli/src/cli.rs" leak: the
        // index page is user-facing copy, not a developer note.
        assert!(!index.contains("crates/once-cli"));
        assert!(!index.contains("build:reference"));
        // Top-level commands appear in sort order.
        let build_pos = index.find("once build").expect("build entry");
        let cache_pos = index.find("once cache").expect("cache entry");
        assert!(build_pos < cache_pos);
    }

    #[test]
    fn committed_mcp_tools_reference_matches_the_server_catalog() {
        let committed = include_str!("../../../docs/reference/mcp/tools.md");
        assert_eq!(committed, render_mcp_tools_page());
    }
}
