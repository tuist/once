use super::{Plan, PlanError};

pub(super) struct Topology {
    pub(super) dependents: Vec<Vec<usize>>,
    pub(super) remaining: Vec<usize>,
}

impl Topology {
    pub(super) fn new(plan: &Plan) -> Result<Self, PlanError> {
        let count = plan.nodes.len();
        let mut dependents = vec![Vec::new(); count];
        let remaining = plan.nodes.iter().map(|node| node.deps.len()).collect();
        for (index, node) in plan.nodes.iter().enumerate() {
            for &dependency in &node.deps {
                let Some(children) = dependents.get_mut(dependency) else {
                    return Err(PlanError::BadDep(dependency, count));
                };
                children.push(index);
            }
        }
        let topology = Self {
            dependents,
            remaining,
        };
        topology.validate()?;
        Ok(topology)
    }

    fn validate(&self) -> Result<(), PlanError> {
        let mut remaining = self.remaining.clone();
        let mut ready: Vec<_> = remaining
            .iter()
            .enumerate()
            .filter_map(|(index, &count)| (count == 0).then_some(index))
            .collect();
        let mut visited = 0;
        while let Some(index) = ready.pop() {
            visited += 1;
            for &dependent in &self.dependents[index] {
                remaining[dependent] -= 1;
                if remaining[dependent] == 0 {
                    ready.push(dependent);
                }
            }
        }
        if visited != remaining.len() {
            return Err(PlanError::Cycle);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, PlanNode, WorkspacePath};

    fn node(deps: Vec<usize>) -> PlanNode {
        PlanNode {
            label: "node".into(),
            action: Action::WriteFile {
                path: WorkspacePath::try_from("out").unwrap(),
                bytes: Vec::new(),
                input_digest: None,
            },
            deps,
        }
    }

    #[test]
    fn duplicate_edges_are_counted_consistently() {
        let plan = Plan {
            nodes: vec![node(vec![]), node(vec![0, 0])],
        };
        let topology = Topology::new(&plan).unwrap();
        assert_eq!(topology.dependents, vec![vec![1, 1], vec![]]);
        assert_eq!(topology.remaining, vec![0, 2]);
    }

    #[test]
    fn a_cycle_beside_an_acyclic_component_is_rejected() {
        let plan = Plan {
            nodes: vec![node(vec![]), node(vec![2]), node(vec![1])],
        };
        assert!(matches!(plan.validate(), Err(PlanError::Cycle)));
    }

    #[test]
    fn a_self_dependency_is_rejected() {
        let plan = Plan {
            nodes: vec![node(vec![0])],
        };
        assert!(matches!(plan.validate(), Err(PlanError::Cycle)));
    }

    #[test]
    fn a_deep_chain_does_not_use_the_call_stack() {
        let plan = Plan {
            nodes: (0_usize..20_000)
                .map(|index| node(index.checked_sub(1).into_iter().collect()))
                .collect(),
        };
        plan.validate().unwrap();
    }
}
