use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

use self::template::{Prompt, RenderedTemplate, Template, Validation};

mod catalog;
mod human;
mod template;

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Template id to instantiate. Omit to choose interactively.
    #[arg(value_name = "TEMPLATE")]
    pub template: Option<String>,

    /// Destination directory, relative to the selected workspace.
    #[arg(long, value_name = "DIR")]
    pub path: Option<PathBuf>,

    /// Provide a prompt answer without interactive input. Repeatable.
    #[arg(long = "set", value_parser = parse_assignment, value_name = "KEY=VALUE")]
    pub values: Vec<(String, String)>,

    /// Print the vendored template catalog and exit.
    #[arg(long)]
    pub templates: bool,

    /// Disable interactive prompts. Missing values surface as output.
    #[arg(long)]
    pub no_input: bool,

    /// Overwrite generated files if they already exist.
    #[arg(long)]
    pub force: bool,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum InitResponse {
    Catalog {
        templates: Vec<TemplateView>,
    },
    SelectTemplate {
        templates: Vec<TemplateView>,
    },
    NeedsInput {
        template: TemplateView,
        prompts: Vec<PromptView>,
        provided: BTreeMap<String, String>,
    },
    Created {
        template: TemplateView,
        destination: String,
        files: Vec<String>,
        values: BTreeMap<String, String>,
        next_steps: Vec<String>,
    },
}

#[derive(Serialize)]
struct TemplateView {
    id: String,
    name: String,
    toolchain: String,
    description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    prompts: Vec<PromptView>,
    next_steps: Vec<String>,
}

#[derive(Serialize)]
struct PromptView {
    name: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    validation: Validation,
}

pub async fn run(workspace: &Path, args: InitArgs, format: Format) -> Result<ExitCode> {
    let templates = catalog::load().context("loading init templates")?;
    if args.templates {
        write_response(
            format,
            human::render_catalog(&templates),
            &InitResponse::Catalog {
                templates: templates
                    .iter()
                    .map(|template| template_view(template, None))
                    .collect(),
            },
        )
        .await?;
        return Ok(ExitCode::SUCCESS);
    }

    let interactive = format == Format::Human
        && !args.no_input
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal();
    let Some(template) = select_template(&templates, args.template.as_deref(), interactive)? else {
        let human_body = format!(
            "{}Run `fabrik init <template-id>` or rerun in an interactive terminal.\n",
            human::render_catalog(&templates)
        );
        write_response(
            format,
            human_body,
            &InitResponse::SelectTemplate {
                templates: templates
                    .iter()
                    .map(|template| template_view(template, None))
                    .collect(),
            },
        )
        .await?;
        return Ok(ExitCode::from(1));
    };

    let mut values = collect_assignments(args.values);
    template.validate_provided_keys(&values)?;
    if interactive {
        let mut stdin = std::io::stdin().lock();
        let mut stderr = std::io::stderr().lock();
        human::collect_values(template, &mut values, &mut stdin, &mut stderr)
            .context("collecting init answers")?;
    } else {
        let resolved = template.resolve_noninteractive_values(&values)?;
        if !resolved.missing.is_empty() {
            let human_body = human::render_missing_inputs(template, &resolved.missing, true);
            write_response(
                format,
                human_body,
                &InitResponse::NeedsInput {
                    template: template_view(template, None),
                    prompts: resolved
                        .missing
                        .iter()
                        .map(|prompt| prompt_view(prompt.prompt, None))
                        .collect(),
                    provided: resolved.values,
                },
            )
            .await?;
            return Ok(ExitCode::from(1));
        }
        values = resolved.values;
    }

    let rendered = template.render(&values)?;
    let destination = resolve_destination(workspace, args.path.as_deref());
    write_rendered(&destination, &rendered, args.force)?;
    let next_steps = display_next_steps(workspace, &destination, &rendered.next_steps);
    write_response(
        format,
        human::render_created(template, &destination, &next_steps),
        &InitResponse::Created {
            template: template_view(template, None),
            destination: destination.display().to_string(),
            files: rendered
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            values,
            next_steps,
        },
    )
    .await?;
    Ok(ExitCode::SUCCESS)
}

fn select_template<'a>(
    templates: &'a [Template],
    requested: Option<&str>,
    interactive: bool,
) -> Result<Option<&'a Template>> {
    if let Some(requested) = requested {
        return templates
            .iter()
            .find(|template| template.matches_id(requested))
            .with_context(|| format!("unknown init template `{requested}`"))
            .map(Some);
    }
    if !interactive {
        return Ok(None);
    }

    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr().lock();
    let selected = human::choose_template(templates, &mut stdin, &mut stderr)?;
    templates
        .iter()
        .find(|template| template.matches_id(&selected))
        .with_context(|| format!("unknown init template `{selected}`"))
        .map(Some)
}

fn collect_assignments(values: Vec<(String, String)>) -> BTreeMap<String, String> {
    let mut assignments = BTreeMap::new();
    for (key, value) in values {
        assignments.insert(key, value);
    }
    assignments
}

fn resolve_destination(workspace: &Path, path: Option<&Path>) -> PathBuf {
    match path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => workspace.join(path),
        None => workspace.to_path_buf(),
    }
}

fn display_next_steps(workspace: &Path, destination: &Path, steps: &[String]) -> Vec<String> {
    let mut rendered = Vec::new();
    if destination != workspace {
        rendered.push(format!("cd {}", destination.display()));
    }
    rendered.extend(steps.iter().cloned());
    rendered
}

fn write_rendered(destination: &Path, rendered: &RenderedTemplate, force: bool) -> Result<()> {
    if destination.exists() {
        if !destination.is_dir() {
            bail!("destination `{}` is not a directory", destination.display());
        }
    } else {
        std::fs::create_dir_all(destination)
            .with_context(|| format!("creating {}", destination.display()))?;
    }

    for file in &rendered.files {
        let path = destination.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if path.exists() && !force {
            bail!(
                "refusing to overwrite `{}`; rerun with `--force`",
                path.display()
            );
        }
        std::fs::write(&path, &file.contents)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

async fn write_response(format: Format, human_body: String, response: &InitResponse) -> Result<()> {
    let body = match format {
        Format::Human => human_body,
        Format::Json | Format::Toon => render::structured(format, response)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn template_view(template: &Template, defaults: Option<&BTreeMap<String, String>>) -> TemplateView {
    TemplateView {
        id: template.id().to_string(),
        name: template.name().to_string(),
        toolchain: template.toolchain().to_string(),
        description: template.description().to_string(),
        aliases: template.aliases().to_vec(),
        prompts: template
            .prompts()
            .iter()
            .map(|prompt| {
                let default =
                    defaults.and_then(|values| prompt.render_default(values).ok().flatten());
                prompt_view(prompt, default.or_else(|| prompt.default.clone()))
            })
            .collect(),
        next_steps: template.next_steps().to_vec(),
    }
}

fn prompt_view(prompt: &Prompt, default: Option<String>) -> PromptView {
    PromptView {
        name: prompt.name.clone(),
        prompt: prompt.question.clone(),
        description: prompt.description.clone(),
        default,
        validation: prompt.validation,
    }
}

fn parse_assignment(raw: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{raw}`"))?;
    if key.trim().is_empty() {
        return Err(format!("expected KEY=VALUE, got `{raw}`"));
    }
    Ok((key.trim().to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_assignment_rejects_missing_equals() {
        let err = parse_assignment("broken").unwrap_err();
        assert!(err.contains("KEY=VALUE"));
    }

    #[test]
    fn resolve_destination_uses_workspace_when_path_is_missing() {
        let workspace = Path::new("/tmp/fabrik");
        assert_eq!(resolve_destination(workspace, None), workspace);
    }

    #[test]
    fn display_next_steps_prefixes_cd_for_subdirectories() {
        let workspace = Path::new("/tmp/fabrik");
        let destination = Path::new("/tmp/fabrik/examples/app");
        let next_steps = display_next_steps(workspace, destination, &["fabrik build hello".into()]);

        assert_eq!(
            next_steps,
            vec![
                "cd /tmp/fabrik/examples/app".to_string(),
                "fabrik build hello".to_string()
            ]
        );
    }
}
