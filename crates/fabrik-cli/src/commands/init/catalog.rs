use anyhow::Result;

use super::template::{BuiltinTemplateSource, Template, TemplateAssetSource};

macro_rules! builtin_template {
    ($root:literal, [$($asset:literal),+ $(,)?]) => {
        BuiltinTemplateSource {
            manifest: include_str!(concat!("templates/", $root, "/template.toml")),
            assets: &[
                $(
                    TemplateAssetSource {
                        path: $asset,
                        contents: include_str!(concat!("templates/", $root, "/", $asset)),
                    },
                )+
            ],
        }
    };
}

const BUILTIN_TEMPLATES: &[BuiltinTemplateSource] = &[
    builtin_template!(
        "rust-granular",
        [
            "fabrik.toml.tmpl",
            "src/main.rs.tmpl",
            "src/lib.rs.tmpl",
            "tests/library.rs.tmpl",
            "README.md.tmpl",
            "mise.toml.tmpl",
        ]
    ),
    builtin_template!(
        "elixir-basic",
        [
            "fabrik.toml.tmpl",
            "lib/entry.ex.tmpl",
            "lib/library.ex.tmpl",
            "README.md.tmpl",
            "mise.toml.tmpl",
        ]
    ),
    builtin_template!(
        "go-basic",
        [
            "fabrik.toml.tmpl",
            "go.mod.tmpl",
            "main.go.tmpl",
            "README.md.tmpl",
            "mise.toml.tmpl",
        ]
    ),
    builtin_template!(
        "apple-macos-cli",
        [
            "fabrik.toml.tmpl",
            "Sources/main.swift.tmpl",
            "Sources/Greeter.swift.tmpl",
            "README.md.tmpl",
        ]
    ),
    builtin_template!(
        "apple-ios-simulator",
        [
            "fabrik.toml.tmpl",
            "Sources/App.swift.tmpl",
            "README.md.tmpl",
        ]
    ),
];

pub(super) fn load() -> Result<Vec<Template>> {
    BUILTIN_TEMPLATES
        .iter()
        .map(Template::from_source)
        .collect()
}
