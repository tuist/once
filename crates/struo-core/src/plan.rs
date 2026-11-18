//! Action graph and parallel scheduler.
//!
//! A [`Plan`] is a DAG of actions with dependency indices. [`Runner::run_plan`]
//! executes ready nodes within a bounded task window and the runner's resource
//! budget. After the first failure it drains running work without starting more.

use crate::{Action, Outcome, Runner};

mod scheduler;
mod topology;

/// One action and its dependencies in a [`Plan`].
#[derive(Debug, Clone)]
pub struct PlanNode {
    /// Opaque human-readable label for telemetry and CLI output.
    pub label: String,
    pub action: Action,
    /// Indices of actions that must succeed before this action runs.
    pub deps: Vec<usize>,
}

/// A directed acyclic graph of actions.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub nodes: Vec<PlanNode>,
}

/// A node's result, returned in plan order regardless of completion order.
#[derive(Debug, Clone)]
pub struct PlanOutcome {
    pub index: usize,
    pub label: String,
    pub outcome: Outcome,
}

/// Per-node metadata for telemetry and CLI rendering.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Project-root-relative id of the source target.
    pub label: String,
    /// Originating target kind, kept opaque to the scheduler.
    pub kind: String,
}

/// A plugin's compiled build description.
#[derive(Debug, Clone)]
pub struct BuiltPlan {
    pub plan: Plan,
    pub root_index: usize,
    /// Project-root-relative id of the root target.
    pub root_id: String,
    /// Workspace-relative canonical output, empty when none is declared.
    pub output: String,
    /// Per-node metadata, indexed in `plan.nodes` order.
    pub nodes: Vec<NodeInfo>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("plan dependency cycle detected")]
    Cycle,
    #[error("dependency index {0} is out of bounds for plan with {1} nodes")]
    BadDep(usize, usize),
    #[error("action `{label}` failed: exit code {exit_code}")]
    Failed { label: String, exit_code: i32 },
    #[error("plan node task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Core(#[from] crate::Error),
}

impl Plan {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Append a node and return its stable index.
    pub fn push(&mut self, node: PlanNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(node);
        idx
    }

    /// Validate dependency indices and detect cycles before any action runs.
    pub fn validate(&self) -> std::result::Result<(), PlanError> {
        topology::Topology::new(self).map(|_| ())
    }
}

impl Runner {
    /// Execute nodes in dependency order, with ready nodes running concurrently.
    /// After the first failure, wait for in-flight actions without launching more.
    pub async fn run_plan(&self, plan: &Plan) -> std::result::Result<Vec<PlanOutcome>, PlanError> {
        scheduler::run(plan, self.max_parallel_plan_tasks(), |node| {
            let runner = self.clone();
            let action = node.action.clone();
            async move { Box::pin(runner.run(&action)).await }
        })
        .await
    }
}

#[cfg(test)]
mod tests;
