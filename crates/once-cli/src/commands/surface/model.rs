use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::Serialize;
use usage::spec::{ArgMeta, CommandMeta, FlagMeta};

use crate::cli::Cli;

#[derive(Clone, Debug, Serialize)]
pub(super) struct CommandSurface {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<ArgSurface>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<CommandSurface>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ArgKind {
    Flag,
    Option,
    Positional,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ArgSurface {
    pub id: String,
    pub syntax: String,
    pub kind: ArgKind,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

pub(super) fn load(path: &[&str]) -> Result<CommandSurface> {
    let selected = select_command(Cli::spec().root, path).context("selecting command surface")?;
    Ok(build_command_surface(selected, &BTreeSet::new()))
}

fn select_command<'a>(command: &'a CommandMeta<'a>, path: &[&str]) -> Result<&'a CommandMeta<'a>> {
    if let Some((head, tail)) = path.split_first() {
        let next = command
            .subcommands
            .iter()
            .copied()
            .find(|subcommand| subcommand.cmd.name == *head)
            .with_context(|| format!("unknown command path segment `{head}`"))?;
        return select_command(next, tail);
    }
    Ok(command)
}

fn build_command_surface(
    command: &CommandMeta<'_>,
    inherited_globals: &BTreeSet<String>,
) -> CommandSurface {
    let mut globals = inherited_globals.clone();
    let mut args = command
        .flags
        .iter()
        .filter(|flag| !flag.hide && !flag.builtin)
        .filter(|flag| !flag.flag.global || !inherited_globals.contains(flag.flag.name))
        .map(|flag| {
            if flag.flag.global {
                globals.insert(flag.flag.name.to_string());
            }
            build_flag_surface(flag)
        })
        .collect::<Vec<_>>();
    args.extend(
        command
            .args
            .iter()
            .filter(|arg| !arg.hide)
            .map(build_positional_surface),
    );
    let subcommands = command
        .subcommands
        .iter()
        .copied()
        .filter(|subcommand| !subcommand.hide)
        .map(|subcommand| build_command_surface(subcommand, &globals))
        .collect::<Vec<_>>();
    CommandSurface {
        name: command.cmd.name.to_string(),
        about: command.about.map(ToString::to_string),
        aliases: command
            .cmd
            .aliases
            .iter()
            .filter(|alias| !command.hidden_aliases.contains(alias))
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        args,
        subcommands,
    }
}

fn build_flag_surface(flag: &FlagMeta<'_>) -> ArgSurface {
    let kind = if flag.flag.takes_value {
        ArgKind::Option
    } else {
        ArgKind::Flag
    };
    ArgSurface {
        id: flag.flag.name.to_string(),
        syntax: flag_syntax(flag),
        kind,
        required: flag.required,
        help: flag.help.map(ToString::to_string),
    }
}

fn build_positional_surface(arg: &ArgMeta<'_>) -> ArgSurface {
    let value = arg
        .value_names
        .first()
        .copied()
        .unwrap_or(arg.arg.name)
        .to_uppercase();
    ArgSurface {
        id: arg.arg.name.to_string(),
        syntax: if arg.required {
            format!("<{value}>")
        } else {
            format!("[{value}]")
        },
        kind: ArgKind::Positional,
        required: arg.required,
        help: arg.help.map(ToString::to_string),
    }
}

fn flag_syntax(flag: &FlagMeta<'_>) -> String {
    let mut parts = Vec::new();
    for short in flag.flag.shorts {
        parts.push(format!("-{}", *short as char));
    }
    for long in flag.flag.longs {
        parts.push(format!("--{long}"));
    }
    let mut syntax = parts.join(", ");
    if flag.flag.takes_value {
        let value = flag
            .value_name
            .or_else(|| flag.value_names.first().copied())
            .unwrap_or("VALUE");
        syntax.push(' ');
        syntax.push('<');
        syntax.push_str(value);
        syntax.push('>');
    }
    syntax
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn parse(argv: &[&str]) -> crate::cli::Cli {
        let argv = argv.iter().map(OsStr::new).collect::<Vec<_>>();
        crate::cli::Cli::try_parse_from(&argv).unwrap()
    }

    #[test]
    fn root_surface_includes_run_and_global_list_flag() {
        let surface = load(&[]).unwrap();
        assert!(surface
            .subcommands
            .iter()
            .any(|command| command.name == "run"));
        assert!(surface.args.iter().any(|arg| arg.syntax.contains("--list")));
    }

    #[test]
    fn cache_subtree_resolves_to_blob_command() {
        let surface = load(&["cache"]).unwrap();
        assert_eq!(surface.name, "cache");
        assert!(surface
            .subcommands
            .iter()
            .any(|command| command.name == "blob"));
    }

    #[test]
    fn unknown_path_returns_error() {
        let err = load(&["does-not-exist"]).unwrap_err();
        assert!(format!("{err:#}").contains("unknown command path segment `does-not-exist`"));
    }

    #[test]
    fn every_subcommand_surface_path_resolves() {
        // Guards against drift between `Cmd::surface_path` and the
        // declared command names. A mismatch otherwise
        // only surfaces at runtime as "unknown command path segment"
        // when `<subcommand> --list` is invoked.
        let invocations: &[&[&str]] = &[
            &["once", "build", "apps/ios/App"],
            &["once", "run", "t"],
            &["once", "exec", "true"],
            &["once", "test", "apps/ios/AppTests"],
            &["once", "cache", "stats"],
            &["once", "cache", "gc", "--max-size", "1GB"],
            &["once", "cache", "blob", "put", "-"],
            &[
                "once",
                "cache",
                "blob",
                "get",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ],
            &[
                "once",
                "cache",
                "blob",
                "exists",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ],
            &[
                "once",
                "cache",
                "action",
                "get",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ],
            &["once", "query", "targets"],
            &["once", "query", "rules"],
            &["once", "query", "capabilities", "apps/ios/App"],
            &["once", "query", "schema", "apple_application"],
            &["once", "toolchain", "inspect"],
            &["once", "runtime", "rpc", "/tmp/session"],
        ];
        for argv in invocations {
            let cli = parse(argv);
            let path = cli.surface_path();
            let surface = load(&path).unwrap_or_else(|e| {
                panic!("resolving surface for {argv:?} (path {path:?}): {e:#}")
            });
            let expected = path.last().copied().unwrap_or("once");
            assert_eq!(surface.name, expected, "argv {argv:?} path {path:?}");
        }
    }
}
