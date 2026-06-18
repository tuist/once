use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{anyhow, bail, Context, Result};
use once_frontend::GraphTarget;
use serde::Serialize;
use serde_json::{json, Map, Value};

const SUPPORTED_LABELS: &[&str] = &["Target", "Capability", "Provider"];
const SUPPORTED_RELATIONSHIPS: &[&str] = &["DEPENDS_ON", "EXPOSES", "EMITS"];

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct QueryResult {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedQuery {
    pattern: MatchPattern,
    predicates: Vec<Predicate>,
    returns: Vec<ReturnItem>,
}

#[derive(Debug, PartialEq, Eq)]
enum MatchPattern {
    Node(NodePattern),
    Relationship {
        left: NodePattern,
        relationship: RelationshipPattern,
        right: NodePattern,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct NodePattern {
    alias: Option<String>,
    label: Option<String>,
    properties: BTreeMap<String, Value>,
}

#[derive(Debug, PartialEq, Eq)]
struct RelationshipPattern {
    ty: String,
    direction: Direction,
    transitive: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Direction {
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, PartialEq, Eq)]
struct Predicate {
    alias: String,
    property: String,
    value: Value,
}

#[derive(Debug, PartialEq, Eq)]
struct ReturnItem {
    column: String,
    projection: Projection,
}

#[derive(Debug, PartialEq, Eq)]
enum Projection {
    Node { alias: String },
    Property { alias: String, property: String },
}

#[derive(Debug)]
struct GraphModel {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

#[derive(Debug)]
struct Node {
    label: String,
    properties: BTreeMap<String, Value>,
}

#[derive(Debug)]
struct Edge {
    from: usize,
    to: usize,
    ty: String,
}

#[derive(Clone, Debug, Default)]
struct Binding {
    nodes: BTreeMap<String, usize>,
}

pub(crate) fn evaluate(query: &str, graph: &[GraphTarget]) -> Result<QueryResult> {
    validate_cypher_syntax(query)?;
    let query = parse_query(query)?;
    let model = GraphModel::from_graph(graph);
    let bindings = model.evaluate(&query)?;
    let rows = bindings
        .iter()
        .map(|binding| {
            query
                .returns
                .iter()
                .map(|item| model.project(binding, item))
                .collect()
        })
        .collect();
    Ok(QueryResult {
        columns: query
            .returns
            .iter()
            .map(|item| item.column.clone())
            .collect(),
        rows,
    })
}

pub(crate) fn render_human(result: &QueryResult) -> String {
    if result.rows.is_empty() {
        return "query: no rows\n".to_string();
    }
    let mut out = String::from("query:\n");
    out.push_str("  ");
    out.push_str(&result.columns.join(" | "));
    out.push('\n');
    for row in &result.rows {
        let values = row.iter().map(render_value).collect::<Vec<_>>();
        out.push_str("  ");
        out.push_str(&values.join(" | "));
        out.push('\n');
    }
    out
}

fn validate_cypher_syntax(query: &str) -> Result<()> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_cypher::LANGUAGE.into())
        .context("loading Cypher grammar")?;
    let tree = parser.parse(query, None).context("parsing Cypher query")?;
    if tree.root_node().has_error() {
        bail!("query is not valid Cypher syntax");
    }
    Ok(())
}

fn parse_query(raw: &str) -> Result<ParsedQuery> {
    let query = strip_trailing_semicolon(raw.trim());
    let match_pos = keyword_pos(query, "MATCH", 0).ok_or_else(|| {
        anyhow!("query expression must start with a read-only `MATCH ... RETURN ...` query")
    })?;
    if !query[..match_pos].trim().is_empty() {
        bail!("query expression must start with `MATCH`");
    }
    let return_pos = keyword_pos(query, "RETURN", match_pos + "MATCH".len())
        .ok_or_else(|| anyhow!("query expression must include `RETURN`"))?;
    let where_pos =
        keyword_pos(query, "WHERE", match_pos + "MATCH".len()).filter(|pos| *pos < return_pos);

    let pattern_start = match_pos + "MATCH".len();
    let pattern_end = where_pos.unwrap_or(return_pos);
    let pattern = parse_match_pattern(query[pattern_start..pattern_end].trim())?;
    let predicates = if let Some(where_pos) = where_pos {
        parse_predicates(query[where_pos + "WHERE".len()..return_pos].trim())?
    } else {
        Vec::new()
    };
    let returns = parse_returns(query[return_pos + "RETURN".len()..].trim())?;
    Ok(ParsedQuery {
        pattern,
        predicates,
        returns,
    })
}

fn parse_match_pattern(raw: &str) -> Result<MatchPattern> {
    let parts = split_top_level(raw, ',')?;
    if parts.len() != 1 {
        bail!("multiple MATCH patterns are not supported yet");
    }
    let (left, rest) = parse_node_at(parts[0].trim())?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(MatchPattern::Node(left));
    }
    let (relationship, rest) = parse_relationship_at(rest)?;
    let (right, rest) = parse_node_at(rest)?;
    if !rest.trim().is_empty() {
        bail!("only one relationship pattern is supported");
    }
    Ok(MatchPattern::Relationship {
        left,
        relationship,
        right,
    })
}

fn parse_node_at(raw: &str) -> Result<(NodePattern, &str)> {
    let raw = raw.trim_start();
    if !raw.starts_with('(') {
        bail!("expected node pattern");
    }
    let close = matching_delimiter(raw, 0, '(', ')')?;
    let node = parse_node(raw[1..close].trim())?;
    Ok((node, &raw[close + 1..]))
}

fn parse_node(raw: &str) -> Result<NodePattern> {
    let (head, properties) = if let Some(open) = top_level_char(raw, '{') {
        let close = matching_delimiter(raw, open, '{', '}')?;
        if !raw[close + 1..].trim().is_empty() {
            bail!("unexpected text after node property map");
        }
        (&raw[..open], parse_property_map(&raw[open + 1..close])?)
    } else {
        (raw, BTreeMap::new())
    };
    let head = head.trim();
    let (alias, label) = if let Some(colon) = head.find(':') {
        let alias = optional_identifier(head[..colon].trim())?;
        let label = canonical_label(head[colon + 1..].trim())?;
        (alias, Some(label))
    } else {
        (optional_identifier(head)?, None)
    };
    Ok(NodePattern {
        alias,
        label,
        properties,
    })
}

fn parse_relationship_at(raw: &str) -> Result<(RelationshipPattern, &str)> {
    if raw.starts_with("-[") {
        let close = matching_delimiter(raw, 1, '[', ']')?;
        let body = &raw[2..close];
        let rest = raw[close + 1..].trim_start();
        let rest = rest
            .strip_prefix("->")
            .ok_or_else(|| anyhow!("expected `->` after relationship pattern"))?;
        return Ok((parse_relationship_body(body, Direction::LeftToRight)?, rest));
    }
    if raw.starts_with("<-[") {
        let close = matching_delimiter(raw, 2, '[', ']')?;
        let body = &raw[3..close];
        let rest = raw[close + 1..].trim_start();
        let rest = rest
            .strip_prefix('-')
            .ok_or_else(|| anyhow!("expected `-` after reverse relationship pattern"))?;
        return Ok((parse_relationship_body(body, Direction::RightToLeft)?, rest));
    }
    bail!("expected relationship pattern such as `-[:DEPENDS_ON]->`");
}

fn parse_relationship_body(raw: &str, direction: Direction) -> Result<RelationshipPattern> {
    let raw = raw.trim();
    let colon = raw
        .find(':')
        .ok_or_else(|| anyhow!("relationship type is required"))?;
    let after_colon = raw[colon + 1..].trim_start();
    let name_len = after_colon
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || *ch == '_')
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()
        .unwrap_or(0);
    if name_len == 0 {
        bail!("relationship type is required");
    }
    let ty = canonical_relationship(&after_colon[..name_len])?;
    Ok(RelationshipPattern {
        ty,
        direction,
        transitive: after_colon[name_len..].contains('*'),
    })
}

fn parse_property_map(raw: &str) -> Result<BTreeMap<String, Value>> {
    let mut properties = BTreeMap::new();
    if raw.trim().is_empty() {
        return Ok(properties);
    }
    for entry in split_top_level(raw, ',')? {
        let colon = top_level_char(entry, ':')
            .ok_or_else(|| anyhow!("property map entries must use `key: value`"))?;
        let key = identifier(entry[..colon].trim())?;
        let value = parse_literal(entry[colon + 1..].trim())?;
        properties.insert(key, value);
    }
    Ok(properties)
}

fn parse_predicates(raw: &str) -> Result<Vec<Predicate>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    split_top_level_keyword(raw, "AND")
        .into_iter()
        .map(|predicate| {
            let eq = top_level_char(predicate, '=')
                .ok_or_else(|| anyhow!("WHERE predicates must use equality"))?;
            let (alias, property) = parse_property_ref(predicate[..eq].trim())?;
            let value = parse_literal(predicate[eq + 1..].trim())?;
            Ok(Predicate {
                alias,
                property,
                value,
            })
        })
        .collect()
}

fn parse_returns(raw: &str) -> Result<Vec<ReturnItem>> {
    if raw.is_empty() {
        bail!("RETURN must include at least one projection");
    }
    split_top_level(raw, ',')?
        .into_iter()
        .map(|item| {
            let item = item.trim();
            let (projection, column) = if let Some(as_pos) = keyword_pos(item, "AS", 0) {
                (
                    parse_projection(item[..as_pos].trim())?,
                    identifier(item[as_pos + "AS".len()..].trim())?,
                )
            } else {
                (parse_projection(item)?, item.to_string())
            };
            Ok(ReturnItem { column, projection })
        })
        .collect()
}

fn parse_projection(raw: &str) -> Result<Projection> {
    if let Some((alias, property)) = raw.split_once('.') {
        return Ok(Projection::Property {
            alias: identifier(alias.trim())?,
            property: identifier(property.trim())?,
        });
    }
    Ok(Projection::Node {
        alias: identifier(raw.trim())?,
    })
}

fn parse_property_ref(raw: &str) -> Result<(String, String)> {
    let (alias, property) = raw
        .split_once('.')
        .ok_or_else(|| anyhow!("property references must use `alias.property`"))?;
    Ok((identifier(alias.trim())?, identifier(property.trim())?))
}

fn parse_literal(raw: &str) -> Result<Value> {
    if raw.starts_with('"') || raw.starts_with('\'') {
        return parse_string_literal(raw).map(Value::String);
    }
    match raw {
        "true" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "FALSE" => return Ok(Value::Bool(false)),
        "null" | "NULL" => return Ok(Value::Null),
        _ => {}
    }
    if let Ok(value) = raw.parse::<i64>() {
        return Ok(json!(value));
    }
    if let Ok(value) = raw.parse::<f64>() {
        return Ok(json!(value));
    }
    bail!("unsupported literal `{raw}`");
}

fn parse_string_literal(raw: &str) -> Result<String> {
    let mut chars = raw.chars();
    let quote = chars
        .next()
        .ok_or_else(|| anyhow!("empty string literal"))?;
    if !matches!(quote, '"' | '\'') || !raw.ends_with(quote) {
        bail!("unterminated string literal");
    }
    let inner = &raw[quote.len_utf8()..raw.len() - quote.len_utf8()];
    let mut output = String::new();
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            output.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }
    if escaped {
        bail!("unterminated string escape");
    }
    Ok(output)
}

impl GraphModel {
    fn from_graph(graph: &[GraphTarget]) -> Self {
        let mut nodes = Vec::new();
        let mut target_indices = BTreeMap::new();
        for target in graph {
            target_indices.insert(target.label.id.clone(), nodes.len());
            nodes.push(Node {
                label: "Target".to_string(),
                properties: target_properties(target),
            });
        }

        let mut edges = Vec::new();
        for target in graph {
            let Some(&target_index) = target_indices.get(&target.label.id) else {
                continue;
            };
            for dep in &target.deps {
                if let Some(&dep_index) = target_indices.get(dep) {
                    edges.push(Edge {
                        from: target_index,
                        to: dep_index,
                        ty: "DEPENDS_ON".to_string(),
                    });
                }
            }
            for capability in &target.capabilities {
                let capability_index = nodes.len();
                nodes.push(Node {
                    label: "Capability".to_string(),
                    properties: BTreeMap::from([
                        ("name".to_string(), json!(capability.name)),
                        ("target".to_string(), json!(target.label.id)),
                        ("output_groups".to_string(), json!(capability.output_groups)),
                        (
                            "requires_outputs".to_string(),
                            json!(capability.requires_outputs),
                        ),
                    ]),
                });
                edges.push(Edge {
                    from: target_index,
                    to: capability_index,
                    ty: "EXPOSES".to_string(),
                });
            }
            for provider in &target.providers {
                let provider_index = nodes.len();
                nodes.push(Node {
                    label: "Provider".to_string(),
                    properties: BTreeMap::from([
                        ("name".to_string(), json!(provider)),
                        ("target".to_string(), json!(target.label.id)),
                    ]),
                });
                edges.push(Edge {
                    from: target_index,
                    to: provider_index,
                    ty: "EMITS".to_string(),
                });
            }
        }

        Self { nodes, edges }
    }

    fn evaluate(&self, query: &ParsedQuery) -> Result<Vec<Binding>> {
        let bindings = match &query.pattern {
            MatchPattern::Node(node) => self.match_node_pattern(node),
            MatchPattern::Relationship {
                left,
                relationship,
                right,
            } => self.match_relationship_pattern(left, relationship, right),
        };
        bindings
            .into_iter()
            .filter(|binding| {
                query
                    .predicates
                    .iter()
                    .all(|predicate| self.predicate_matches(binding, predicate))
            })
            .map(Ok)
            .collect()
    }

    fn match_node_pattern(&self, pattern: &NodePattern) -> Vec<Binding> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node_matches(node, pattern))
            .map(|(index, _)| bind_node(Binding::default(), pattern, index))
            .collect()
    }

    fn match_relationship_pattern(
        &self,
        left: &NodePattern,
        relationship: &RelationshipPattern,
        right: &NodePattern,
    ) -> Vec<Binding> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node_matches(node, left))
            .flat_map(|(left_index, _)| {
                self.reachable(left_index, relationship)
                    .into_iter()
                    .filter(|right_index| node_matches(&self.nodes[*right_index], right))
                    .map(move |right_index| {
                        let binding = bind_node(Binding::default(), left, left_index);
                        bind_node(binding, right, right_index)
                    })
            })
            .collect()
    }

    fn reachable(&self, start: usize, relationship: &RelationshipPattern) -> Vec<usize> {
        if !relationship.transitive {
            return self.neighbors(start, relationship).collect();
        }
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([start]);
        let mut results = Vec::new();
        seen.insert(start);
        while let Some(node) = queue.pop_front() {
            for next in self.neighbors(node, relationship) {
                if seen.insert(next) {
                    results.push(next);
                    queue.push_back(next);
                }
            }
        }
        results
    }

    fn neighbors<'a>(
        &'a self,
        start: usize,
        relationship: &'a RelationshipPattern,
    ) -> impl Iterator<Item = usize> + 'a {
        self.edges.iter().filter_map(move |edge| {
            if edge.ty != relationship.ty {
                return None;
            }
            match relationship.direction {
                Direction::LeftToRight if edge.from == start => Some(edge.to),
                Direction::RightToLeft if edge.to == start => Some(edge.from),
                _ => None,
            }
        })
    }

    fn predicate_matches(&self, binding: &Binding, predicate: &Predicate) -> bool {
        let Some(node) = binding.nodes.get(&predicate.alias) else {
            return false;
        };
        self.nodes[*node].properties.get(&predicate.property) == Some(&predicate.value)
    }

    fn project(&self, binding: &Binding, item: &ReturnItem) -> Value {
        match &item.projection {
            Projection::Node { alias } => binding.nodes.get(alias).map_or(Value::Null, |index| {
                object_value(&self.nodes[*index].properties)
            }),
            Projection::Property { alias, property } => binding
                .nodes
                .get(alias)
                .and_then(|index| self.nodes[*index].properties.get(property))
                .cloned()
                .unwrap_or(Value::Null),
        }
    }
}

fn target_properties(target: &GraphTarget) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("id".to_string(), json!(target.label.id)),
        ("package".to_string(), json!(target.label.package)),
        ("name".to_string(), json!(target.label.name)),
        ("kind".to_string(), json!(target.kind)),
        ("deps".to_string(), json!(target.deps)),
        ("srcs".to_string(), json!(target.srcs)),
        ("attrs".to_string(), json!(target.attrs)),
        ("providers".to_string(), json!(target.providers)),
        (
            "capabilities".to_string(),
            json!(target
                .capabilities
                .iter()
                .map(|capability| capability.name.as_str())
                .collect::<Vec<_>>()),
        ),
    ])
}

fn node_matches(node: &Node, pattern: &NodePattern) -> bool {
    if pattern
        .label
        .as_ref()
        .is_some_and(|label| node.label != *label)
    {
        return false;
    }
    pattern.properties.iter().all(|(key, value)| {
        node.properties
            .get(key)
            .is_some_and(|actual| actual == value)
    })
}

fn bind_node(mut binding: Binding, pattern: &NodePattern, index: usize) -> Binding {
    if let Some(alias) = &pattern.alias {
        binding.nodes.insert(alias.clone(), index);
    }
    binding
}

fn object_value(properties: &BTreeMap<String, Value>) -> Value {
    Value::Object(
        properties
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<_, _>>(),
    )
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn optional_identifier(raw: &str) -> Result<Option<String>> {
    if raw.is_empty() {
        Ok(None)
    } else {
        identifier(raw).map(Some)
    }
}

fn identifier(raw: &str) -> Result<String> {
    if raw.is_empty() {
        bail!("identifier is required");
    }
    let mut chars = raw.chars();
    let first = chars
        .next()
        .ok_or_else(|| anyhow!("identifier is required"))?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        bail!("invalid identifier `{raw}`");
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        bail!("invalid identifier `{raw}`");
    }
    Ok(raw.to_string())
}

fn canonical_label(raw: &str) -> Result<String> {
    let label = SUPPORTED_LABELS
        .iter()
        .find(|label| label.eq_ignore_ascii_case(raw))
        .ok_or_else(|| {
            anyhow!(
                "unsupported graph label `{raw}`; supported labels: {}",
                SUPPORTED_LABELS.join(", ")
            )
        })?;
    Ok((*label).to_string())
}

fn canonical_relationship(raw: &str) -> Result<String> {
    let relationship = SUPPORTED_RELATIONSHIPS
        .iter()
        .find(|relationship| relationship.eq_ignore_ascii_case(raw))
        .ok_or_else(|| {
            anyhow!(
                "unsupported relationship `{raw}`; supported relationships: {}",
                SUPPORTED_RELATIONSHIPS.join(", ")
            )
        })?;
    Ok((*relationship).to_string())
}

fn strip_trailing_semicolon(raw: &str) -> &str {
    raw.strip_suffix(';').map_or(raw, str::trim_end)
}

fn keyword_pos(input: &str, keyword: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices().skip_while(|(idx, _)| *idx < start) {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if input[idx..]
            .get(..keyword.len())
            .is_some_and(|part| part.eq_ignore_ascii_case(keyword))
            && keyword_boundary(input, idx, keyword.len())
        {
            return Some(idx);
        }
    }
    None
}

fn keyword_boundary(input: &str, start: usize, len: usize) -> bool {
    let before = input[..start].chars().next_back();
    let after = input[start + len..].chars().next();
    before.is_none_or(|ch| !is_identifier_char(ch))
        && after.is_none_or(|ch| !is_identifier_char(ch))
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn split_top_level(input: &str, separator: char) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ch if ch == separator && depth == 0 => {
                parts.push(input[start..idx].trim());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
        if depth < 0 {
            bail!("unbalanced delimiters");
        }
    }
    if quote.is_some() {
        bail!("unterminated string literal");
    }
    if depth != 0 {
        bail!("unbalanced delimiters");
    }
    parts.push(input[start..].trim());
    Ok(parts)
}

fn split_top_level_keyword<'a>(input: &'a str, keyword: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut start = 0;
    while let Some(pos) = keyword_pos(input, keyword, start) {
        parts.push(input[start..pos].trim());
        start = pos + keyword.len();
    }
    parts.push(input[start..].trim());
    parts
}

fn top_level_char(input: &str, needle: char) -> Option<usize> {
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ch if ch == needle && depth == 0 => return Some(idx),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn matching_delimiter(input: &str, open: usize, left: char, right: char) -> Result<usize> {
    if !input[open..].starts_with(left) {
        bail!("expected `{left}`");
    }
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices().skip_while(|(idx, _)| *idx < open) {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ch if ch == left => depth += 1,
            ch if ch == right => {
                depth -= 1;
                if depth == 0 {
                    return Ok(idx);
                }
            }
            _ => {}
        }
    }
    bail!("unclosed `{left}`");
}

#[cfg(test)]
mod tests {
    use once_frontend::{Capability, TargetLabel};

    use super::*;

    fn target(id: &str, kind: &str, deps: &[&str], capabilities: &[&str]) -> GraphTarget {
        let (package, name) = id.rsplit_once('/').unwrap_or(("", id));
        GraphTarget {
            label: TargetLabel {
                package: package.to_string(),
                name: name.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: capabilities
                .iter()
                .map(|name| Capability {
                    name: (*name).to_string(),
                    output_groups: vec!["default".to_string()],
                    requires_outputs: Vec::new(),
                })
                .collect(),
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn evaluates_transitive_dependency_query() {
        let graph = vec![
            target("apps/App", "apple_application", &["libs/Core"], &["build"]),
            target("libs/Core", "apple_library", &["libs/Util"], &["build"]),
            target("libs/Util", "apple_library", &[], &["build"]),
        ];

        let result = evaluate(
            r#"MATCH (app:Target {id: "apps/App"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind"#,
            &graph,
        )
        .unwrap();

        assert_eq!(result.columns, vec!["dep.id", "dep.kind"]);
        assert_eq!(
            result.rows,
            vec![
                vec![json!("libs/Core"), json!("apple_library")],
                vec![json!("libs/Util"), json!("apple_library")],
            ]
        );
    }

    #[test]
    fn evaluates_capability_query_with_where() {
        let graph = vec![
            target("apps/App", "apple_application", &[], &["build", "run"]),
            target(
                "apps/AppTests",
                "apple_test_bundle",
                &[],
                &["build", "test"],
            ),
        ];

        let result = evaluate(
            r#"MATCH (t:Target)-[:EXPOSES]->(c:Capability) WHERE c.name = "test" RETURN t.id AS target"#,
            &graph,
        )
        .unwrap();

        assert_eq!(result.columns, vec!["target"]);
        assert_eq!(result.rows, vec![vec![json!("apps/AppTests")]]);
    }

    #[test]
    fn rejects_invalid_cypher() {
        let error = evaluate("MATCH (t:Target RETURN t.id", &[]).unwrap_err();
        assert!(error.to_string().contains("not valid Cypher syntax"));
    }

    #[test]
    fn rejects_unsupported_relationships() {
        let error = evaluate("MATCH (a:Target)-[:OWNS]->(b:Target) RETURN b.id", &[]).unwrap_err();
        assert!(error.to_string().contains("unsupported relationship"));
    }

    #[test]
    fn renders_human_rows() {
        let rendered = render_human(&QueryResult {
            columns: vec!["t.id".to_string()],
            rows: vec![vec![json!("apps/App")]],
        });
        assert_eq!(rendered, "query:\n  t.id\n  apps/App\n");
    }
}
