//! Parses supported read-only Cypher into a [`ParsedQuery`] and rejects
//! unsupported or mutating statements. Cypher syntax is validated with
//! the tree-sitter grammar; the supported subset is then read out of the
//! source text with the scanning helpers in [`super::scan`].

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use super::ast::{
    Direction, MatchPattern, NodePattern, ParsedQuery, Predicate, PredicateOperator, Projection,
    RelationshipPattern, ReturnItem,
};
use super::scan::{
    keyword_pos, matching_delimiter, split_top_level, split_top_level_keyword, starts_with_keyword,
    strip_trailing_semicolon, top_level_char,
};

const SUPPORTED_LABELS: &[&str] = &["Target", "Capability", "Provider"];
const SUPPORTED_RELATIONSHIPS: &[&str] = &["DEPENDS_ON", "EXPOSES", "EMITS"];
const UNSUPPORTED_STATEMENT_PREFIXES: &[&str] = &["DROP"];
const UNSUPPORTED_CLAUSE_NODES: &[(&str, &str)] = &[
    ("call_clause", "CALL"),
    ("create_clause", "CREATE"),
    ("delete_clause", "DELETE"),
    ("merge_clause", "MERGE"),
    ("remove_clause", "REMOVE"),
    ("set_clause", "SET"),
    ("unwind_clause", "UNWIND"),
    ("with_clause", "WITH"),
];

pub(super) fn parse_cypher(query: &str) -> Result<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_cypher::LANGUAGE.into())
        .context("loading Cypher grammar")?;
    let tree = parser.parse(query, None).context("parsing Cypher query")?;
    if tree.root_node().has_error() {
        bail!("query is not valid Cypher syntax");
    }
    Ok(tree)
}

pub(super) fn parse_query(raw: &str) -> Result<ParsedQuery> {
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
    let predicate_groups = if let Some(where_pos) = where_pos {
        parse_predicates(query[where_pos + "WHERE".len()..return_pos].trim())?
    } else {
        Vec::new()
    };
    let returns = parse_returns(query[return_pos + "RETURN".len()..].trim())?;
    Ok(ParsedQuery {
        pattern,
        predicate_groups,
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
    reject_duplicate_relationship_aliases(&left, &right)?;
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

fn parse_predicates(raw: &str) -> Result<Vec<Vec<Predicate>>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    split_top_level_keyword(raw, "OR")
        .into_iter()
        .map(|group| {
            split_top_level_keyword(group, "AND")
                .into_iter()
                .map(parse_predicate)
                .collect()
        })
        .collect()
}

fn parse_predicate(raw: &str) -> Result<Predicate> {
    let operators = [
        ("STARTS WITH", PredicateOperator::StartsWith),
        ("ENDS WITH", PredicateOperator::EndsWith),
        ("CONTAINS", PredicateOperator::Contains),
        ("IN", PredicateOperator::In),
    ];
    for (keyword, operator) in operators {
        if let Some(position) = keyword_pos(raw, keyword, 0) {
            return predicate_from_parts(
                &raw[..position],
                &raw[position + keyword.len()..],
                operator,
            );
        }
    }
    if let Some(position) = top_level_operator(raw, "<>") {
        return predicate_from_parts(
            &raw[..position],
            &raw[position + 2..],
            PredicateOperator::NotEqual,
        );
    }
    if let Some(position) = top_level_char(raw, '=') {
        return predicate_from_parts(
            &raw[..position],
            &raw[position + 1..],
            PredicateOperator::Equal,
        );
    }
    bail!("WHERE predicates must use =, <>, CONTAINS, IN, STARTS WITH, or ENDS WITH")
}

fn predicate_from_parts(
    property: &str,
    value: &str,
    operator: PredicateOperator,
) -> Result<Predicate> {
    let (alias, property) = parse_property_ref(property.trim())?;
    Ok(Predicate {
        alias,
        property,
        operator,
        value: parse_literal(value.trim())?,
    })
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
            property: property_path(property.trim())?,
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
    Ok((identifier(alias.trim())?, property_path(property.trim())?))
}

fn parse_literal(raw: &str) -> Result<Value> {
    if raw.starts_with('[') && raw.ends_with(']') {
        let inner = &raw[1..raw.len() - 1];
        if inner.trim().is_empty() {
            return Ok(Value::Array(Vec::new()));
        }
        return Ok(Value::Array(
            split_top_level(inner, ',')?
                .into_iter()
                .map(parse_literal)
                .collect::<Result<Vec<_>>>()?,
        ));
    }
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

fn property_path(raw: &str) -> Result<String> {
    raw.split('.')
        .map(str::trim)
        .map(identifier)
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("."))
}

fn top_level_operator(input: &str, operator: &str) -> Option<usize> {
    let first = operator.chars().next()?;
    let mut start = 0;
    while let Some(relative) = top_level_char(&input[start..], first) {
        let position = start + relative;
        if input[position..].starts_with(operator) {
            return Some(position);
        }
        start = position + first.len_utf8();
    }
    None
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
            let value = match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                other => bail!("unsupported string escape `\\{other}`"),
            };
            output.push(value);
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

pub(super) fn reject_unsupported_statement_prefixes(query: &str) -> Result<()> {
    let query = strip_trailing_semicolon(query.trim_start());
    for clause in UNSUPPORTED_STATEMENT_PREFIXES {
        if starts_with_keyword(query, clause) {
            bail!("query expressions are read-only; `{clause}` is not supported");
        }
    }
    Ok(())
}

pub(super) fn reject_unsupported_clauses(tree: &tree_sitter::Tree) -> Result<()> {
    let mut nodes = vec![tree.root_node()];
    while let Some(node) = nodes.pop() {
        if let Some((_, clause)) = UNSUPPORTED_CLAUSE_NODES
            .iter()
            .find(|(kind, _)| node.kind() == *kind)
        {
            bail!("query expressions are read-only; `{clause}` is not supported");
        }
        if node.kind() == "match_clause" && match_clause_is_optional(node) {
            bail!("query expressions are read-only; `OPTIONAL` is not supported");
        }
        for index in (0..node.child_count())
            .rev()
            .filter_map(|index| u32::try_from(index).ok())
        {
            if let Some(child) = node.child(index) {
                nodes.push(child);
            }
        }
    }
    Ok(())
}

fn match_clause_is_optional(node: tree_sitter::Node<'_>) -> bool {
    (0..node.child_count())
        .filter_map(|index| u32::try_from(index).ok())
        .any(|index| {
            node.child(index)
                .is_some_and(|child| child.kind() == "optional")
        })
}

fn reject_duplicate_relationship_aliases(left: &NodePattern, right: &NodePattern) -> Result<()> {
    if let (Some(left), Some(right)) = (&left.alias, &right.alias) {
        if left == right {
            bail!("duplicate relationship alias `{left}` is not supported");
        }
    }
    Ok(())
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
