//! Serializable views that MCP tools return for graph and target kind queries.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct TargetKindSummary {
    pub(super) kind: String,
    pub(super) docs: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) source_references: Vec<struo_frontend::SourceReference>,
    pub(super) examples: Vec<TargetKindExampleSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct TargetKindExampleSummary {
    pub(super) slug: String,
    pub(super) name: String,
    pub(super) use_when: String,
}

impl From<struo_frontend::TargetKindSchema> for TargetKindSummary {
    fn from(schema: struo_frontend::TargetKindSchema) -> Self {
        Self {
            kind: schema.kind,
            docs: schema.docs,
            source_references: schema.source_references,
            examples: schema
                .examples
                .into_iter()
                .map(|example| TargetKindExampleSummary {
                    slug: example.slug,
                    name: example.name,
                    use_when: example.use_when,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct TargetView {
    pub(super) id: String,
    pub(super) package: String,
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) deps: Vec<String>,
    pub(super) dependency_edges: std::collections::BTreeMap<String, Vec<String>>,
    pub(super) visibility: Vec<String>,
    pub(super) capabilities: Vec<String>,
}

impl From<struo_frontend::GraphTarget> for TargetView {
    fn from(target: struo_frontend::GraphTarget) -> Self {
        Self {
            id: target.label.id,
            package: target.label.package,
            name: target.label.name,
            kind: target.kind,
            deps: target.deps,
            dependency_edges: target.dependency_edges,
            visibility: target.visibility,
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| capability.name)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct CapabilityView {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) capabilities: Vec<CapabilityDetail>,
}

#[derive(Debug, Serialize)]
pub(super) struct CapabilityDetail {
    pub(super) name: String,
    pub(super) output_groups: Vec<String>,
    pub(super) requires_outputs: Vec<String>,
}

impl From<struo_frontend::GraphTarget> for CapabilityView {
    fn from(target: struo_frontend::GraphTarget) -> Self {
        Self {
            id: target.label.id,
            kind: target.kind,
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| CapabilityDetail {
                    name: capability.name,
                    output_groups: capability.output_groups,
                    requires_outputs: capability.requires_outputs,
                })
                .collect(),
        }
    }
}
