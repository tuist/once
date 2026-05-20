use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{bail, Result};
use console::Style;

use super::template::{MissingPrompt, Template};

pub(super) fn render_catalog(templates: &[Template]) -> String {
    let mut body = format!("{}\n\n", stdout_heading("Available templates"));
    let mut current_toolchain = None;
    for template in templates {
        if current_toolchain != Some(template.toolchain()) {
            if current_toolchain.is_some() {
                body.push('\n');
            }
            current_toolchain = Some(template.toolchain());
            body.push_str(&stdout_group(template.toolchain()));
            body.push('\n');
        }
        body.push_str("  ");
        body.push_str(&stdout_id(template.id()));
        body.push_str("  ");
        body.push_str(template.description());
        body.push('\n');
    }
    body
}

pub(super) fn render_missing_inputs(
    template: &Template,
    missing: &[MissingPrompt<'_>],
    interactive_hint: bool,
) -> String {
    let mut body = format!(
        "{} {}\n",
        stdout_warning("Template requires more input:"),
        stdout_id(template.id())
    );
    for prompt in missing {
        body.push_str("  ");
        body.push_str(&stdout_key(&prompt.prompt.name));
        body.push(' ');
        body.push_str(&prompt.prompt.question);
        if let Some(description) = &prompt.prompt.description {
            body.push_str(" (");
            body.push_str(description);
            body.push(')');
        }
        body.push('\n');
    }
    if interactive_hint {
        body.push_str("Run ");
        body.push_str(&stdout_command(&format!("fabrik init {}", template.id())));
        body.push_str(" in an interactive terminal, or pass answers with ");
        body.push_str(&stdout_command("--set key=value"));
        body.push_str(".\n");
    } else {
        body.push_str("Pass answers with ");
        body.push_str(&stdout_command("--set key=value"));
        body.push_str(".\n");
    }
    body
}

pub(super) fn render_created(
    template: &Template,
    destination: &Path,
    next_steps: &[String],
) -> String {
    let mut body = format!(
        "{} {} in {}\n",
        stdout_success("Created"),
        stdout_name(template.name()),
        stdout_path(&destination.display().to_string())
    );
    if !next_steps.is_empty() {
        body.push_str(&stdout_heading("Next steps"));
        body.push('\n');
        for step in next_steps {
            body.push_str("  ");
            body.push_str(&stdout_command(step));
            body.push('\n');
        }
    }
    body
}

pub(super) fn choose_template<R: BufRead, W: Write>(
    templates: &[Template],
    input: &mut R,
    output: &mut W,
) -> Result<String> {
    writeln!(output, "{}", stderr_heading("Available templates"))?;
    for (index, template) in templates.iter().enumerate() {
        writeln!(
            output,
            "{} {} {}",
            index + 1,
            stderr_id(template.id()),
            stderr_dim(&format!("({})", template.toolchain()))
        )?;
        writeln!(output, "   {}", template.description())?;
    }
    loop {
        write!(output, "{} ", stderr_prompt("Template id or number:"))?;
        output.flush()?;
        let raw = read_line(input)?;
        if raw.is_empty() {
            writeln!(
                output,
                "{}",
                stderr_warning("Select a template id or number.")
            )?;
            continue;
        }
        if let Ok(index) = raw.parse::<usize>() {
            if (1..=templates.len()).contains(&index) {
                return Ok(templates[index - 1].id().to_string());
            }
        }
        if let Some(template) = templates.iter().find(|template| template.matches_id(&raw)) {
            return Ok(template.id().to_string());
        }
        writeln!(
            output,
            "{} {}",
            stderr_error("Unknown template:"),
            stderr_id(&raw)
        )?;
    }
}

pub(super) fn collect_values<R: BufRead, W: Write>(
    template: &Template,
    values: &mut BTreeMap<String, String>,
    input: &mut R,
    output: &mut W,
) -> Result<()> {
    template.validate_provided_keys(values)?;
    for prompt in template.prompts() {
        if let Some(raw) = values.get(&prompt.name).cloned() {
            let normalized = prompt.validate(&raw)?;
            values.insert(prompt.name.clone(), normalized);
            continue;
        }

        let default = prompt.render_default(values)?;
        loop {
            if let Some(description) = &prompt.description {
                writeln!(output, "{}", stderr_dim(description))?;
            }
            write!(output, "{}", stderr_prompt(&prompt.question))?;
            if let Some(default) = &default {
                write!(output, " {}", stderr_dim(&format!("[default: {default}]")))?;
            }
            write!(output, ": ")?;
            output.flush()?;

            let raw = read_line(input)?;
            let candidate = if raw.is_empty() {
                default.clone().unwrap_or_default()
            } else {
                raw
            };
            match prompt.validate(&candidate) {
                Ok(value) => {
                    values.insert(prompt.name.clone(), value);
                    break;
                }
                Err(err) => writeln!(output, "{}", stderr_error(&err.to_string()))?,
            }
        }
    }
    Ok(())
}

// `console` already honors `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`,
// and tty detection, gating stdout and stderr independently. These
// helpers only pick the stream and the palette; `stdout_*` is for text
// written to stdout, `stderr_*` for the interactive prompt stream.
fn paint(value: &str, for_stderr: bool, style: impl FnOnce(Style) -> Style) -> String {
    let base = if for_stderr {
        Style::new().for_stderr()
    } else {
        Style::new()
    };
    style(base).apply_to(value).to_string()
}

fn stdout_heading(value: &str) -> String {
    paint(value, false, Style::bold)
}

fn stdout_group(value: &str) -> String {
    paint(&title_case(value), false, |s| s.cyan().bold())
}

fn stdout_id(value: &str) -> String {
    paint(value, false, |s| s.green().bold())
}

fn stdout_name(value: &str) -> String {
    paint(value, false, Style::bold)
}

fn stdout_path(value: &str) -> String {
    paint(value, false, Style::cyan)
}

fn stdout_success(value: &str) -> String {
    paint(value, false, |s| s.green().bold())
}

fn stdout_command(value: &str) -> String {
    paint(value, false, Style::yellow)
}

fn stdout_warning(value: &str) -> String {
    paint(value, false, |s| s.yellow().bold())
}

fn stdout_key(value: &str) -> String {
    paint(value, false, Style::bold)
}

fn stderr_heading(value: &str) -> String {
    paint(value, true, Style::bold)
}

fn stderr_prompt(value: &str) -> String {
    paint(value, true, |s| s.cyan().bold())
}

fn stderr_warning(value: &str) -> String {
    paint(value, true, |s| s.yellow().bold())
}

fn stderr_error(value: &str) -> String {
    paint(value, true, |s| s.red().bold())
}

fn stderr_id(value: &str) -> String {
    paint(value, true, |s| s.green().bold())
}

fn stderr_dim(value: &str) -> String {
    paint(value, true, Style::dim)
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn read_line<R: BufRead>(input: &mut R) -> Result<String> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        bail!("reached end of input while collecting init answers");
    }
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;

    use super::*;
    use crate::commands::init::catalog;

    #[test]
    fn choose_template_accepts_numeric_selection() {
        let templates = catalog::load().unwrap();
        let mut input = Cursor::new("2\n");
        let mut output = Vec::new();

        let selected = choose_template(&templates, &mut input, &mut output).unwrap();

        assert_eq!(selected, "elixir-app");
    }

    #[test]
    fn collect_values_uses_prompt_defaults() {
        let templates = catalog::load().unwrap();
        let template = templates
            .iter()
            .find(|template| template.id() == "rust-app")
            .unwrap();
        let mut values = BTreeMap::from([
            ("project_name".to_string(), "hello".to_string()),
            ("library_name".to_string(), "greeting".to_string()),
        ]);
        let mut input = Cursor::new("\n\n");
        let mut output = Vec::new();

        collect_values(template, &mut values, &mut input, &mut output).unwrap();

        assert_eq!(values["test_name"], "greeting_test");
        assert_eq!(values["greeting_subject"], "Rust");
    }
}
