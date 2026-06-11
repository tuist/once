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
    write_command_files(out, &bin_name, &entries)?;
    write_index(out, &bin_name, &entries)?;

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

fn write_command_files(out: &Path, bin: &str, entries: &[CommandEntry]) -> Result<()> {
    for entry in entries {
        let mut body = String::new();
        let command_path = format!("{bin} {}", entry.path.join(" "));
        writeln!(&mut body, "# `{command_path}`\n").ok();
        if let Some(about) = &entry.about {
            writeln!(&mut body, "{about}\n").ok();
        }

        // Synopsis line: positional + a `[OPTIONS]` placeholder when
        // there are any non-positional flags.
        let mut synopsis = command_path.clone();
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
        writeln!(&mut body, "## Synopsis\n").ok();
        writeln!(&mut body, "```text\n{synopsis}\n```\n").ok();

        // clap's doc-comment derive copies the short summary into the
        // long-about, so the first paragraph of `long_about` usually
        // restates `about`. Render only the trailing prose so the
        // page doesn't show the same sentence twice (the about line
        // already prints above the synopsis).
        if let Some(long) = entry
            .long_about
            .as_deref()
            .and_then(|l| trim_leading_about(l, entry.about.as_deref()))
        {
            writeln!(&mut body, "## Description\n").ok();
            writeln!(&mut body, "{long}\n").ok();
        }

        if !entry.positionals.is_empty() {
            writeln!(&mut body, "## Arguments\n").ok();
            writeln!(&mut body, "| Argument | Required | Description |").ok();
            writeln!(&mut body, "| --- | --- | --- |").ok();
            for positional in &entry.positionals {
                writeln!(
                    &mut body,
                    "| `<{name}>` | {required} | {description} |",
                    name = positional.name,
                    required = if positional.required { "yes" } else { "no" },
                    description = positional.description.as_deref().unwrap_or("")
                )
                .ok();
            }
            body.push('\n');
        }

        if !entry.options.is_empty() {
            writeln!(&mut body, "## Options\n").ok();
            writeln!(&mut body, "| Flag | Value | Default | Description |").ok();
            writeln!(&mut body, "| --- | --- | --- | --- |").ok();
            for opt in &entry.options {
                let value = if opt.takes_value {
                    format!("`<{}>`", opt.value_name.as_deref().unwrap_or("VALUE"))
                } else {
                    "(flag)".to_string()
                };
                let default = match opt.default.as_deref().filter(|d| !d.is_empty()) {
                    Some(d) => format!("`{d}`"),
                    None => String::new(),
                };
                writeln!(
                    &mut body,
                    "| `{name}` | {value} | {default} | {description} |",
                    name = opt.name,
                    value = value,
                    default = default,
                    description = opt.description.as_deref().unwrap_or("")
                )
                .ok();
            }
            body.push('\n');
        }

        if !entry.children.is_empty() {
            writeln!(&mut body, "## Subcommands\n").ok();
            for child in &entry.children {
                let mut child_path = entry.path.clone();
                child_path.push(child.clone());
                let child_link = format!("/reference/cli/{}", child_path.join("/"));
                writeln!(
                    &mut body,
                    "- [`{bin} {} {child}`]({child_link})",
                    entry.path.join(" ")
                )
                .ok();
                let _ = child_link;
            }
            body.push('\n');
        }

        let path = file_path_for(out, &entry.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating reference subdir `{}`", parent.display()))?;
        }
        fs::write(&path, body)
            .with_context(|| format!("writing reference page `{}`", path.display()))?;
    }
    Ok(())
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

fn write_index(out: &Path, bin: &str, entries: &[CommandEntry]) -> Result<()> {
    let mut body = String::new();
    writeln!(&mut body, "# CLI Reference\n").ok();
    writeln!(
        &mut body,
        "Generated from the `clap` definitions in `crates/once-cli/src/cli.rs`. Re-run `npm run build:reference` after touching that file so this section stays current.\n"
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
