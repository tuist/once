use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use super::graph::{Ecosystem, ResolvedDependency, ResolvedGraph, ResolvedPackage, ResolvedSource};

pub(super) async fn load_graph(lockfile: &Path) -> Result<ResolvedGraph> {
    let body = tokio::fs::read_to_string(lockfile)
        .await
        .with_context(|| format!("reading {}", lockfile.display()))?;
    Ok(parse_mix_lock(&body))
}

fn parse_mix_lock(body: &str) -> ResolvedGraph {
    let mut packages = Vec::new();
    for entry in top_level_entries(body) {
        let Some((name, value)) = parse_entry(entry) else {
            continue;
        };
        let fields = tuple_fields(value);
        match fields.first().map(|field| field.trim()) {
            Some(":hex") if fields.len() >= 8 => {
                packages.push(hex_package(name, &fields));
            }
            Some(":git") if fields.len() >= 3 => {
                packages.push(git_package(&name, &fields));
            }
            _ => {}
        }
    }
    ResolvedGraph::new(Ecosystem::Elixir, packages)
}

fn hex_package(lock_key: String, fields: &[&str]) -> ResolvedPackage {
    let package_name = parse_atom(fields[1].trim()).unwrap_or_else(|| lock_key.clone());
    let version = parse_string(fields[2].trim());
    let inner_checksum = parse_string(fields[3].trim());
    let dependencies = parse_dependency_list(fields[5].trim());
    let repo = parse_string(fields[6].trim());
    let outer_checksum = parse_string(fields[7].trim());
    let checksum = outer_checksum.or(inner_checksum);
    let id = version.as_deref().map_or_else(
        || package_name.clone(),
        |version| format!("{package_name}@{version}"),
    );
    let metadata = package_metadata(package_name.as_str(), lock_key);

    ResolvedPackage {
        id,
        name: package_name,
        version,
        source: ResolvedSource::Registry { registry: repo },
        checksum,
        dependencies,
        metadata,
    }
}

fn git_package(lock_key: &str, fields: &[&str]) -> ResolvedPackage {
    let url = parse_string(fields[1].trim()).unwrap_or_default();
    let revision = parse_string(fields[2].trim());
    let id = revision.as_deref().map_or_else(
        || lock_key.to_string(),
        |revision| format!("{lock_key}#{revision}"),
    );
    ResolvedPackage {
        id,
        name: lock_key.to_string(),
        version: None,
        source: ResolvedSource::Git { url, revision },
        checksum: None,
        dependencies: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

fn package_metadata(package_name: &str, lock_key: String) -> BTreeMap<String, serde_json::Value> {
    let mut metadata = BTreeMap::new();
    if package_name != lock_key {
        metadata.insert("lock_key".to_string(), serde_json::Value::String(lock_key));
    }
    metadata
}

fn parse_entry(entry: &str) -> Option<(String, &str)> {
    let entry = entry.trim();
    let (key, rest) = parse_quoted_prefix(entry)?;
    let rest = rest.trim_start();
    let value = rest
        .strip_prefix(':')
        .or_else(|| rest.strip_prefix("=>"))?
        .trim_start();
    Some((key, value))
}

fn top_level_entries(body: &str) -> Vec<&str> {
    let body = body.trim();
    let body = body
        .strip_prefix("%{")
        .unwrap_or(body)
        .trim()
        .strip_suffix('}')
        .unwrap_or(body)
        .trim();
    split_top_level(body, ',')
}

fn tuple_fields(value: &str) -> Vec<&str> {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) else {
        return Vec::new();
    };
    split_top_level(inner, ',')
}

fn parse_dependency_list(value: &str) -> Vec<ResolvedDependency> {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Vec::new();
    };
    split_top_level(inner, ',')
        .into_iter()
        .filter_map(|dep| {
            let dep = dep.trim();
            let dep_inner = dep.strip_prefix('{')?.strip_suffix('}')?;
            let fields = split_top_level(dep_inner, ',');
            let name = parse_atom(fields.first()?.trim())?;
            Some(ResolvedDependency {
                id: name.clone(),
                name,
                kind: "normal".to_string(),
            })
        })
        .collect()
}

fn split_top_level(input: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            _ if ch == separator && depth == 0 => {
                parts.push(input[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

fn parse_quoted_prefix(input: &str) -> Option<(String, &str)> {
    let mut chars = input.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some((input[1..index].to_string(), &input[index + 1..]));
        }
    }
    None
}

fn parse_string(input: &str) -> Option<String> {
    parse_quoted_prefix(input).map(|(value, _)| value)
}

fn parse_atom(input: &str) -> Option<String> {
    let input = input.strip_prefix(':')?;
    if let Some((quoted, _)) = parse_quoted_prefix(input) {
        return Some(quoted);
    }
    let atom: String = input
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect();
    if atom.is_empty() {
        None
    } else {
        Some(atom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_packages_from_mix_lock() {
        let graph = parse_mix_lock(
            r#"%{
  "decimal": {:hex, :decimal, "2.1.1", "inner", [:mix], [], "hexpm", "outer"},
  "jason": {:hex, :jason, "1.4.1", "inner2", [:mix], [{:decimal, "~> 2.0", [hex: :decimal, repo: "hexpm", optional: true]}], "hexpm", "outer2"}
}"#,
        );

        assert_eq!(graph.ecosystem, Ecosystem::Elixir);
        assert_eq!(graph.packages.len(), 2);
        let jason = graph
            .packages
            .iter()
            .find(|pkg| pkg.name == "jason")
            .unwrap();
        assert_eq!(jason.id, "jason@1.4.1");
        assert_eq!(jason.dependencies[0].name, "decimal");
        assert_eq!(jason.checksum.as_deref(), Some("outer2"));
    }

    #[test]
    fn accepts_arrow_map_entries() {
        let graph = parse_mix_lock(
            r#"%{
  "plug" => {:hex, :plug, "1.16.1", "inner", [:mix], [], "hexpm", "outer"}
}"#,
        );

        assert_eq!(graph.packages[0].name, "plug");
    }

    #[test]
    fn parses_git_entries_from_mix_lock() {
        let graph = parse_mix_lock(
            r#"%{
  "cowboy": {:git, "https://github.com/ninenines/cowboy.git", "abc123", [tag: "2.10.0"]}
}"#,
        );

        assert_eq!(graph.packages[0].id, "cowboy#abc123");
        assert_eq!(
            graph.packages[0].source,
            ResolvedSource::Git {
                url: "https://github.com/ninenines/cowboy.git".to_string(),
                revision: Some("abc123".to_string())
            }
        );
    }
}
