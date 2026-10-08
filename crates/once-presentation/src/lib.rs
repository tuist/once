use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ActionPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<ActionPackage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<ActionPlatform>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ActionContext>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ActionPackage {
    pub ecosystem: String,
    pub name: String,
    pub version: String,
    pub revision: String,
    pub digest: String,
    pub origin: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ActionPlatform {
    pub scheme: String,
    pub id: String,
    pub label: String,
    pub usage: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ActionContext {
    pub key: String,
    pub value: String,
    pub label: String,
}

impl ActionPresentation {
    #[must_use]
    pub fn normalized(mut self) -> Option<Self> {
        self.package = self.package.filter(|package| {
            token(&package.ecosystem)
                && bounded(&package.name, 128)
                && [&package.version, &package.revision, &package.digest]
                    .iter()
                    .all(|value| value.is_empty() || bounded(value, 256))
                && (package.origin.is_empty() || token(&package.origin))
        });
        self.platforms.retain(|platform| {
            token(&platform.scheme)
                && bounded(&platform.id, 256)
                && (platform.usage.is_empty() || token(&platform.usage))
        });
        self.platforms.truncate(8);
        for platform in &mut self.platforms {
            platform.label = display(&platform.label);
        }
        self.context
            .retain(|context| token(&context.key) && bounded(&context.value, 256));
        self.context.truncate(8);
        for context in &mut self.context {
            context.label = display(&context.label);
        }
        while self.metadata_bytes() > 2048 {
            if self.context.pop().is_some() {
                continue;
            }
            if self.platforms.pop().is_some() {
                continue;
            }
            self.package = None;
        }
        (self.package.is_some() || !self.platforms.is_empty() || !self.context.is_empty())
            .then_some(self)
    }

    fn metadata_bytes(&self) -> usize {
        let package = self.package.as_ref().map_or(0, |package| {
            [
                &package.ecosystem,
                &package.name,
                &package.version,
                &package.revision,
                &package.digest,
                &package.origin,
            ]
            .iter()
            .map(|value| value.len())
            .sum::<usize>()
        });
        package
            + self
                .platforms
                .iter()
                .map(|platform| {
                    [
                        &platform.scheme,
                        &platform.id,
                        &platform.label,
                        &platform.usage,
                    ]
                    .iter()
                    .map(|value| value.len())
                    .sum::<usize>()
                })
                .sum::<usize>()
            + self
                .context
                .iter()
                .map(|context| {
                    [&context.key, &context.value, &context.label]
                        .iter()
                        .map(|value| value.len())
                        .sum::<usize>()
                })
                .sum::<usize>()
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn bounded(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn display(value: &str) -> String {
    if value.len() > 128 {
        return String::new();
    }
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_future_metadata_are_optional() {
        let legacy: ActionPresentation = serde_json::from_str("{}").unwrap();
        assert!(legacy.normalized().is_none());
        let value: ActionPresentation = serde_json::from_str(
            r#"{"platforms":[{"scheme":"future","id":"target","usage":"future","new":true}]}"#,
        )
        .unwrap();
        assert_eq!(value.normalized().unwrap().platforms[0].usage, "future");
    }

    #[test]
    fn preserves_ids_and_bounds_presentation() {
        let presentation = ActionPresentation {
            platforms: vec![ActionPlatform {
                scheme: "native".into(),
                id: "x".repeat(257),
                ..Default::default()
            }],
            context: (0..20)
                .map(|_| ActionContext {
                    key: "custom.mode".into(),
                    value: "test".into(),
                    label: "Mode\ntest".into(),
                })
                .collect(),
            ..Default::default()
        }
        .normalized()
        .unwrap();
        assert!(presentation.platforms.is_empty());
        assert_eq!(presentation.context.len(), 8);
        assert_eq!(presentation.context[0].label, "Mode test");
    }
}
