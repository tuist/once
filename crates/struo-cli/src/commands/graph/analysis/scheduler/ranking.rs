use std::collections::{HashMap, HashSet};

use anyhow::Result;

pub(super) fn critical_depths(
    reachable: &HashSet<String>,
    dependents: &HashMap<String, Vec<String>>,
) -> Result<HashMap<String, usize>> {
    let mut remaining = HashMap::with_capacity(reachable.len());
    let mut predecessors: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut ready = Vec::new();
    for target_id in reachable {
        let mut count = 0;
        if let Some(next_targets) = dependents.get(target_id) {
            for next_id in next_targets
                .iter()
                .filter(|next_id| reachable.contains(*next_id))
            {
                count += 1;
                predecessors.entry(next_id).or_default().push(target_id);
            }
        }
        remaining.insert(target_id.as_str(), count);
        if count == 0 {
            ready.push(target_id.as_str());
        }
    }

    let mut depths = HashMap::with_capacity(reachable.len());
    let mut pending_depths: HashMap<&str, usize> = HashMap::new();
    while let Some(target_id) = ready.pop() {
        let depth = pending_depths.remove(target_id).unwrap_or(1);
        depths.insert(target_id.to_string(), depth);
        if let Some(previous_targets) = predecessors.get(target_id) {
            for &previous in previous_targets {
                let previous_depth = pending_depths.entry(previous).or_insert(1);
                *previous_depth = (*previous_depth).max(depth.saturating_add(1));
                let count = remaining
                    .get_mut(previous)
                    .expect("predecessor is reachable");
                *count -= 1;
                if *count == 0 {
                    ready.push(previous);
                }
            }
        }
    }
    if depths.len() != reachable.len() {
        let target_id = reachable
            .iter()
            .filter(|target| !depths.contains_key(*target))
            .min()
            .expect("an unranked target remains");
        anyhow::bail!("cycle detected while ranking graph target `{target_id}`");
    }
    Ok(depths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_depth_prioritizes_the_longest_remaining_chain() {
        let reachable = ["root", "short", "long-a", "long-b"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let dependents = HashMap::from([
            ("short".to_string(), vec!["root".to_string()]),
            ("long-a".to_string(), vec!["long-b".to_string()]),
            ("long-b".to_string(), vec!["root".to_string()]),
        ]);

        let depths = critical_depths(&reachable, &dependents).unwrap();

        assert_eq!(depths["root"], 1);
        assert_eq!(depths["short"], 2);
        assert_eq!(depths["long-b"], 2);
        assert_eq!(depths["long-a"], 3);
    }

    #[test]
    fn critical_depth_rejects_cycles() {
        let reachable = ["a", "b", "leaf"].into_iter().map(str::to_string).collect();
        let dependents = HashMap::from([
            ("a".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
        ]);
        let error = critical_depths(&reachable, &dependents).unwrap_err();
        assert_eq!(
            error.to_string(),
            "cycle detected while ranking graph target `a`"
        );
    }

    #[test]
    fn critical_depth_handles_duplicate_and_unreachable_dependents() {
        let reachable = ["a", "b", "isolated"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let dependents = HashMap::from([
            (
                "a".to_string(),
                vec!["b".to_string(), "b".to_string(), "outside".to_string()],
            ),
            ("outside".to_string(), vec!["a".to_string()]),
        ]);
        let depths = critical_depths(&reachable, &dependents).unwrap();
        assert_eq!(depths["a"], 2);
        assert_eq!(depths["b"], 1);
        assert_eq!(depths["isolated"], 1);
    }

    #[test]
    fn critical_depth_handles_deep_graphs_without_recursion() {
        let labels: Vec<_> = (0..20_000).map(|index| index.to_string()).collect();
        let reachable = labels.iter().cloned().collect();
        let dependents = labels
            .windows(2)
            .map(|pair| (pair[0].clone(), vec![pair[1].clone()]))
            .collect();
        let depths = critical_depths(&reachable, &dependents).unwrap();
        for (index, label) in labels.iter().enumerate() {
            assert_eq!(depths[label], labels.len() - index);
        }
    }
}
