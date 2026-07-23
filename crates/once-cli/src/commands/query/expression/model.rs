//! In-memory graph built from the resolved targets, plus evaluation of a
//! [`ParsedQuery`] against it. Targets, their capabilities, and their
//! providers become labelled nodes; dependencies and ownership become
//! typed edges that the relationship patterns traverse.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::Result;
use once_frontend::GraphTarget;
use serde_json::{json, Map, Value};

use super::ast::{
    Direction, MatchPattern, NodePattern, ParsedQuery, Predicate, PredicateOperator, Projection,
    RelationshipPattern, ReturnItem,
};

#[derive(Debug)]
pub(super) struct GraphModel {
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
pub(super) struct Binding {
    nodes: BTreeMap<String, usize>,
}

impl GraphModel {
    pub(super) fn from_graph(graph: &[GraphTarget]) -> Self {
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
            for dep in target.dependency_ids() {
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

    pub(super) fn evaluate(&self, query: &ParsedQuery) -> Result<Vec<Binding>> {
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
                query.predicate_groups.is_empty()
                    || query.predicate_groups.iter().any(|group| {
                        group
                            .iter()
                            .all(|predicate| self.predicate_matches(binding, predicate))
                    })
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
        let actual = property_value(&self.nodes[*node].properties, &predicate.property);
        match predicate.operator {
            PredicateOperator::Equal => actual == Some(&predicate.value),
            PredicateOperator::NotEqual => actual != Some(&predicate.value),
            PredicateOperator::Contains => actual.is_some_and(|actual| match actual {
                Value::String(actual) => predicate
                    .value
                    .as_str()
                    .is_some_and(|expected| actual.contains(expected)),
                Value::Array(actual) => actual.contains(&predicate.value),
                _ => false,
            }),
            PredicateOperator::In => predicate
                .value
                .as_array()
                .is_some_and(|values| actual.is_some_and(|actual| values.contains(actual))),
            PredicateOperator::StartsWith => {
                string_predicate(actual, &predicate.value, |actual, expected| {
                    actual.starts_with(expected)
                })
            }
            PredicateOperator::EndsWith => {
                string_predicate(actual, &predicate.value, |actual, expected| {
                    actual.ends_with(expected)
                })
            }
        }
    }

    pub(super) fn project(&self, binding: &Binding, item: &ReturnItem) -> Value {
        match &item.projection {
            Projection::Node { alias } => binding.nodes.get(alias).map_or(Value::Null, |index| {
                object_value(&self.nodes[*index].properties)
            }),
            Projection::Property { alias, property } => binding
                .nodes
                .get(alias)
                .and_then(|index| property_value(&self.nodes[*index].properties, property))
                .cloned()
                .unwrap_or(Value::Null),
        }
    }
}

fn property_value<'a>(properties: &'a BTreeMap<String, Value>, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut value = properties.get(parts.next()?)?;
    for part in parts {
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

fn string_predicate(
    actual: Option<&Value>,
    expected: &Value,
    predicate: impl Fn(&str, &str) -> bool,
) -> bool {
    actual
        .and_then(Value::as_str)
        .zip(expected.as_str())
        .is_some_and(|(actual, expected)| predicate(actual, expected))
}

fn target_properties(target: &GraphTarget) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("id".to_string(), json!(target.label.id)),
        ("package".to_string(), json!(target.label.package)),
        ("name".to_string(), json!(target.label.name)),
        ("kind".to_string(), json!(target.kind)),
        ("deps".to_string(), json!(target.deps)),
        (
            "dependency_edges".to_string(),
            json!(target.dependency_edges),
        ),
        ("srcs".to_string(), json!(target.srcs)),
        ("visibility".to_string(), json!(target.visibility)),
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
