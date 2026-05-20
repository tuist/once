use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub(super) struct BuiltinTemplateSource {
    pub manifest: &'static str,
    pub assets: &'static [TemplateAssetSource],
}

pub(super) struct TemplateAssetSource {
    pub path: &'static str,
    pub contents: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct Template {
    assets: BTreeMap<&'static str, &'static str>,
    manifest: Manifest,
}

pub(super) struct ResolvedValues<'a> {
    pub values: BTreeMap<String, String>,
    pub missing: Vec<MissingPrompt<'a>>,
}

pub(super) struct MissingPrompt<'a> {
    pub prompt: &'a Prompt,
}

#[derive(Debug, Clone)]
pub(super) struct RenderedTemplate {
    pub files: Vec<RenderedFile>,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct RenderedFile {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    name: String,
    toolchain: String,
    description: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    prompts: Vec<Prompt>,
    #[serde(default)]
    files: Vec<FileSpec>,
    #[serde(default)]
    next_steps: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prompt {
    pub name: String,
    #[serde(rename = "prompt")]
    pub question: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub validation: Validation,
}

#[derive(Copy, Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Validation {
    #[default]
    LineText,
    KebabCase,
    SnakeCase,
    PascalCase,
    BundleId,
    Version,
    ModulePath,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSpec {
    path: String,
    source: String,
}

impl Template {
    pub(super) fn from_source(source: &BuiltinTemplateSource) -> Result<Self> {
        let manifest: Manifest =
            toml::from_str(source.manifest).context("parsing template manifest")?;
        let assets = source
            .assets
            .iter()
            .map(|asset| (asset.path, asset.contents))
            .collect::<BTreeMap<_, _>>();
        let template = Self { assets, manifest };
        template.validate()?;
        Ok(template)
    }

    pub(super) fn id(&self) -> &str {
        &self.manifest.id
    }

    pub(super) fn name(&self) -> &str {
        &self.manifest.name
    }

    pub(super) fn aliases(&self) -> &[String] {
        &self.manifest.aliases
    }

    pub(super) fn toolchain(&self) -> &str {
        &self.manifest.toolchain
    }

    pub(super) fn description(&self) -> &str {
        &self.manifest.description
    }

    pub(super) fn prompts(&self) -> &[Prompt] {
        &self.manifest.prompts
    }

    pub(super) fn next_steps(&self) -> &[String] {
        &self.manifest.next_steps
    }

    pub(super) fn validate_provided_keys(&self, values: &BTreeMap<String, String>) -> Result<()> {
        let known = self
            .manifest
            .prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<BTreeSet<_>>();
        for key in values.keys() {
            if !known.contains(key.as_str()) {
                bail!(
                    "template `{}` does not define a prompt named `{key}`",
                    self.id()
                );
            }
        }
        Ok(())
    }

    pub(super) fn matches_id(&self, raw: &str) -> bool {
        self.id() == raw || self.manifest.aliases.iter().any(|alias| alias == raw)
    }

    pub(super) fn resolve_noninteractive_values<'a>(
        &'a self,
        provided: &BTreeMap<String, String>,
    ) -> Result<ResolvedValues<'a>> {
        self.validate_provided_keys(provided)?;
        let mut values = BTreeMap::new();
        let mut missing = Vec::new();
        for prompt in &self.manifest.prompts {
            if let Some(raw) = provided.get(&prompt.name) {
                values.insert(prompt.name.clone(), prompt.validate(raw)?);
                continue;
            }
            if let Some(default) = prompt.render_default(&values)? {
                values.insert(prompt.name.clone(), default);
                continue;
            }
            missing.push(MissingPrompt { prompt });
        }
        Ok(ResolvedValues { values, missing })
    }

    pub(super) fn render(&self, values: &BTreeMap<String, String>) -> Result<RenderedTemplate> {
        let mut files = Vec::new();
        let mut seen_paths = BTreeSet::new();
        for file in &self.manifest.files {
            let path = render_text(&file.path, values)
                .with_context(|| format!("rendering file path `{}`", file.path))?;
            validate_relative_path(&path)?;
            if !seen_paths.insert(path.clone()) {
                bail!("template `{}` renders `{path}` more than once", self.id());
            }
            let source = self.assets.get(file.source.as_str()).with_context(|| {
                format!(
                    "missing asset `{}` in template `{}`",
                    file.source,
                    self.id()
                )
            })?;
            let contents =
                render_text(source, values).with_context(|| format!("rendering file `{path}`"))?;
            files.push(RenderedFile { path, contents });
        }
        let next_steps = self
            .manifest
            .next_steps
            .iter()
            .map(|step| render_text(step, values))
            .collect::<Result<Vec<_>>>()?;
        Ok(RenderedTemplate { files, next_steps })
    }

    fn validate(&self) -> Result<()> {
        if self.manifest.id.trim().is_empty() {
            bail!("template id must not be empty");
        }
        let mut aliases = BTreeSet::new();
        for alias in &self.manifest.aliases {
            if alias.trim().is_empty() {
                bail!("template `{}` has an empty alias", self.id());
            }
            if alias == self.id() {
                bail!("template `{}` alias duplicates its id", self.id());
            }
            if !aliases.insert(alias.as_str()) {
                bail!(
                    "template `{}` defines alias `{alias}` more than once",
                    self.id()
                );
            }
        }
        // Defaults are rendered against the values resolved so far, so
        // a `{{key}}` in a default can only resolve if `key` is an
        // earlier prompt. Enforcing that here turns a reorder mistake
        // into a clear manifest error instead of a "missing template
        // value" failure at render time.
        let mut names = BTreeSet::new();
        for prompt in &self.manifest.prompts {
            if prompt.name.trim().is_empty() {
                bail!("template `{}` has an empty prompt name", self.id());
            }
            if let Some(default) = &prompt.default {
                for key in referenced_keys(default).with_context(|| {
                    format!(
                        "template `{}` prompt `{}` default",
                        self.id(),
                        prompt.name
                    )
                })? {
                    if !names.contains(key.as_str()) {
                        bail!(
                            "template `{}` prompt `{}` default references `{key}`, which is not an \
                             earlier prompt; defaults may only reference prompts declared before them",
                            self.id(),
                            prompt.name
                        );
                    }
                }
            }
            if !names.insert(prompt.name.as_str()) {
                bail!(
                    "template `{}` defines prompt `{}` more than once",
                    self.id(),
                    prompt.name
                );
            }
        }
        for file in &self.manifest.files {
            if !self.assets.contains_key(file.source.as_str()) {
                bail!(
                    "template `{}` references missing asset `{}`",
                    self.id(),
                    file.source
                );
            }
        }
        Ok(())
    }
}

impl Prompt {
    /// Render this prompt's default against the values resolved so far.
    /// A default may only reference earlier prompts; manifest
    /// validation enforces that, so a `{{key}}` here is guaranteed to
    /// resolve once those prompts have values.
    pub(super) fn render_default(
        &self,
        values: &BTreeMap<String, String>,
    ) -> Result<Option<String>> {
        match &self.default {
            Some(default) => {
                let rendered = render_text(default, values)
                    .with_context(|| format!("rendering default for prompt `{}`", self.name))?;
                Ok(Some(self.validate(&rendered)?))
            }
            None => Ok(None),
        }
    }

    pub(super) fn validate(&self, value: &str) -> Result<String> {
        let normalized = value.trim().to_string();
        self.validation
            .validate(&normalized)
            .with_context(|| format!("prompt `{}`", self.name))?;
        Ok(normalized)
    }
}

impl Validation {
    fn validate(self, value: &str) -> Result<()> {
        if value.is_empty() {
            bail!("value must not be empty");
        }
        match self {
            Self::LineText => validate_line_text(value),
            Self::KebabCase => validate_case(value, '-').context("expected kebab-case"),
            Self::SnakeCase => validate_case(value, '_').context("expected snake_case"),
            Self::PascalCase => validate_pascal_case(value),
            Self::BundleId => validate_bundle_id(value),
            Self::Version => validate_version(value),
            Self::ModulePath => validate_module_path(value),
        }
    }
}

fn render_text(input: &str, values: &BTreeMap<String, String>) -> Result<String> {
    let mut output = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find("}}")
            .context("found `{{` without a matching `}}`")?;
        let key = tail[..end].trim();
        let value = values
            .get(key)
            .with_context(|| format!("missing template value `{key}`"))?;
        output.push_str(value);
        rest = &tail[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Collect the `{{key}}` names referenced by `input`, in order. Used by
/// manifest validation to reject defaults that reference a prompt which
/// is not declared earlier.
fn referenced_keys(input: &str) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let tail = &rest[start + 2..];
        let end = tail
            .find("}}")
            .context("found `{{` without a matching `}}`")?;
        keys.push(tail[..end].trim().to_string());
        rest = &tail[end + 2..];
    }
    Ok(keys)
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("template rendered an empty path");
    }
    let path = Path::new(path);
    if path.is_absolute() {
        bail!("template file path must be relative");
    }
    for component in path.components() {
        match component {
            Component::CurDir | Component::Normal(_) => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("template file path must stay within the destination directory");
            }
        }
    }
    Ok(())
}

fn validate_line_text(value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        bail!("expected a single-line value");
    }
    if value.contains('"') || value.contains('\\') {
        bail!("quotes and backslashes are not supported in this field");
    }
    Ok(())
}

fn validate_case(value: &str, separator: char) -> Result<()> {
    if value.starts_with(separator) || value.ends_with(separator) {
        bail!("value must not start or end with `{separator}`");
    }
    for part in value.split(separator) {
        if part.is_empty() {
            bail!("value must not contain repeated `{separator}`");
        }
        if !part
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        {
            bail!("value must contain only lowercase ASCII letters and digits");
        }
    }
    Ok(())
}

fn validate_pascal_case(value: &str) -> Result<()> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        bail!("value must not be empty");
    };
    if !first.is_ascii_uppercase() {
        bail!("expected PascalCase");
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric()) {
        bail!("expected only ASCII letters and digits");
    }
    Ok(())
}

fn validate_bundle_id(value: &str) -> Result<()> {
    if value.split('.').count() < 2 {
        bail!("bundle id must contain at least one dot");
    }
    for segment in value.split('.') {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            bail!("bundle id segments must not be empty");
        };
        if !first.is_ascii_alphabetic() {
            bail!("bundle id segments must start with a letter");
        }
        if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
            bail!("bundle id segments may only contain ASCII letters, digits, and `-`");
        }
    }
    Ok(())
}

fn validate_version(value: &str) -> Result<()> {
    if value.split('.').count() < 2 {
        bail!("version must contain at least one dot");
    }
    for part in value.split('.') {
        if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
            bail!("version segments must contain only digits");
        }
    }
    Ok(())
}

fn validate_module_path(value: &str) -> Result<()> {
    if value.contains(char::is_whitespace) {
        bail!("module paths must not contain whitespace");
    }
    if value.contains('"') || value.contains('\\') {
        bail!("quotes and backslashes are not supported in module paths");
    }
    if value.split('/').any(str::is_empty) {
        bail!("module paths must not contain empty path segments");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::commands::init::catalog;

    #[test]
    fn rust_template_derives_test_name_from_library_name() {
        let templates = catalog::load().unwrap();
        let template = templates
            .iter()
            .find(|template| template.id() == "rust-app")
            .unwrap();
        let resolved = template
            .resolve_noninteractive_values(&BTreeMap::from([
                ("project_name".to_string(), "hello".to_string()),
                ("library_name".to_string(), "greeting".to_string()),
                ("greeting_subject".to_string(), "Rust".to_string()),
            ]))
            .unwrap();

        assert!(resolved.missing.is_empty());
        assert_eq!(resolved.values["test_name"], "greeting_test");
    }

    #[test]
    fn templates_reject_unknown_values() {
        let templates = catalog::load().unwrap();
        let template = templates
            .iter()
            .find(|template| template.id() == "go-app")
            .unwrap();
        let err = template
            .validate_provided_keys(&BTreeMap::from([(
                "unexpected".to_string(),
                "value".to_string(),
            )]))
            .unwrap_err();

        assert!(err.to_string().contains("unexpected"));
    }

    #[test]
    fn parent_dirs_are_not_allowed_in_rendered_paths() {
        let err = validate_relative_path("../escape").unwrap_err();
        assert!(err.to_string().contains("destination directory"));
    }

    #[test]
    fn rendered_paths_reject_parent_dirs_from_template_values() {
        let source = BuiltinTemplateSource {
            manifest: r#"
id = "path-check"
name = "Path Check"
toolchain = "test"
description = "tests rendered path validation"

[[prompts]]
name = "segment"
prompt = "Segment"

[[files]]
path = "{{segment}}/main.txt"
source = "main.txt.tmpl"
"#,
            assets: &[TemplateAssetSource {
                path: "main.txt.tmpl",
                contents: "hello",
            }],
        };
        let template = Template::from_source(&source).unwrap();
        let err = template
            .render(&BTreeMap::from([(
                "segment".to_string(),
                "../escape".to_string(),
            )]))
            .unwrap_err();

        assert!(err.to_string().contains("destination directory"));
    }

    #[test]
    fn aliases_match_legacy_ids() {
        let templates = catalog::load().unwrap();
        let template = templates
            .iter()
            .find(|template| template.id() == "ios-app")
            .unwrap();

        assert!(template.matches_id("apple-ios-simulator"));
    }
}
