//! Parsed representation of a supported read-only Cypher query. These
//! types are the shared vocabulary between the parser, which builds them
//! from source text, and the model, which evaluates them against a graph.

use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ParsedQuery {
    pub(super) pattern: MatchPattern,
    pub(super) predicates: Vec<Predicate>,
    pub(super) returns: Vec<ReturnItem>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MatchPattern {
    Node(NodePattern),
    Relationship {
        left: NodePattern,
        relationship: RelationshipPattern,
        right: NodePattern,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct NodePattern {
    pub(super) alias: Option<String>,
    pub(super) label: Option<String>,
    pub(super) properties: BTreeMap<String, Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RelationshipPattern {
    pub(super) ty: String,
    pub(super) direction: Direction,
    pub(super) transitive: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Direction {
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Predicate {
    pub(super) alias: String,
    pub(super) property: String,
    pub(super) value: Value,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReturnItem {
    pub(super) column: String,
    pub(super) projection: Projection,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Projection {
    Node { alias: String },
    Property { alias: String, property: String },
}
