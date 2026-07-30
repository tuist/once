use std::collections::BTreeSet;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider, Digest};
use serde::{Deserialize, Serialize};

use crate::{run_uncached, Action, Result};

/// How many times an action is re-run to test reproducibility. Two is the
/// minimum that can observe nondeterminism: one baseline run and one
/// confirmation run. More trials raise confidence that an intermittent
/// nondeterminism is real rather than a one-off, at the cost of more work.
const TRIALS: usize = 2;

/// Outcome of checking whether an action reproduces its outputs.
///
/// An action is reproducible when repeated runs from the same inputs produce
/// the same exit code, captured streams, and declared outputs. The report
/// records every observed divergence so a caller can explain what changed
/// rather than only that something did.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReproducibilityReport {
    /// True when no divergence was observed across the trials.
    pub reproducible: bool,
    /// Number of fresh executions performed.
    pub trials: usize,
    /// Each divergence found, in a stable order.
    pub differences: Vec<ReproducibilityDifference>,
}

/// One observed divergence between two trials of the same action.
///
/// Tagged so a caller can render each kind distinctly: a differing exit code
/// means the action's outcome is unstable, a differing stream means captured
/// output drifted, and a differing output path means a declared artifact was
/// rewritten or appeared and disappeared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReproducibilityDifference {
    ExitCode {
        first: i32,
        second: i32,
    },
    Stdout {
        first: Option<Digest>,
        second: Option<Digest>,
    },
    Stderr {
        first: Option<Digest>,
        second: Option<Digest>,
    },
    Output {
        path: String,
        first: Option<Digest>,
        second: Option<Digest>,
    },
}

impl ReproducibilityDifference {
    /// One-line, human-readable description of the divergence.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::ExitCode { first, second } => {
                format!("exit code differs: {first} then {second}")
            }
            Self::Stdout { first, second } => {
                format!(
                    "stdout differs: {} then {}",
                    fmt_opt(first.as_ref()),
                    fmt_opt(second.as_ref())
                )
            }
            Self::Stderr { first, second } => {
                format!(
                    "stderr differs: {} then {}",
                    fmt_opt(first.as_ref()),
                    fmt_opt(second.as_ref())
                )
            }
            Self::Output {
                path,
                first,
                second,
            } => format!(
                "output `{path}` differs: {} then {}",
                fmt_opt(first.as_ref()),
                fmt_opt(second.as_ref())
            ),
        }
    }
}

fn fmt_opt(digest: Option<&Digest>) -> String {
    match digest {
        Some(d) => d.to_string(),
        None => "<absent>".to_string(),
    }
}

/// Run an action twice while bypassing the action cache and report whether
/// the two trials produced identical results.
///
/// Each trial runs from a clean execution of the action, never a replay, so
/// the comparison tests whether the outputs are a pure function of the inputs.
/// The action executes `TRIALS` times; callers should expect that cost. The
/// workspace is shared across trials, so for an action that writes declared
/// outputs without a sandbox the second trial overwrites the first trial's
/// outputs. That is fine here because each trial records its own output
/// digests as it produces them, before any later trial runs.
pub async fn verify_reproducible(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ReproducibilityReport> {
    let mut results = Vec::with_capacity(TRIALS);
    for _ in 0..TRIALS {
        results.push(run_uncached(action, workspace_root, cache, false).await?);
    }
    let mut differences = Vec::new();
    for window in results.windows(2) {
        differences.extend(compare(&window[0], &window[1]));
    }
    // Collapse duplicate divergences so three or more trials do not repeat the
    // same finding once per adjacent pair. Two trials never duplicate.
    differences.dedup();
    let reproducible = differences.is_empty();
    Ok(ReproducibilityReport {
        reproducible,
        trials: TRIALS,
        differences,
    })
}

fn compare(first: &ActionResult, second: &ActionResult) -> Vec<ReproducibilityDifference> {
    let mut differences = Vec::new();
    if first.exit_code != second.exit_code {
        differences.push(ReproducibilityDifference::ExitCode {
            first: first.exit_code,
            second: second.exit_code,
        });
    }
    if first.stdout != second.stdout {
        differences.push(ReproducibilityDifference::Stdout {
            first: first.stdout,
            second: second.stdout,
        });
    }
    if first.stderr != second.stderr {
        differences.push(ReproducibilityDifference::Stderr {
            first: first.stderr,
            second: second.stderr,
        });
    }
    let mut paths: BTreeSet<&String> = first.outputs.keys().collect();
    paths.extend(second.outputs.keys());
    for path in paths {
        let a = first.outputs.get(path).copied();
        let b = second.outputs.get(path).copied();
        if a != b {
            differences.push(ReproducibilityDifference::Output {
                path: path.clone(),
                first: a,
                second: b,
            });
        }
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(exit: i32, stdout: Option<Digest>, outputs: &[(&str, Digest)]) -> ActionResult {
        ActionResult {
            exit_code: exit,
            stdout,
            stderr: None,
            outputs: outputs.iter().map(|(p, d)| (p.to_string(), *d)).collect(),
        }
    }

    #[test]
    fn identical_results_are_reproducible() {
        let d = Digest::of_bytes(b"x");
        let differences = compare(
            &result(0, Some(d), &[("out", d)]),
            &result(0, Some(d), &[("out", d)]),
        );
        assert!(differences.is_empty());
    }

    #[test]
    fn differing_exit_code_is_reported() {
        let differences = compare(&result(0, None, &[]), &result(1, None, &[]));
        assert_eq!(differences.len(), 1);
        assert!(matches!(
            differences[0],
            ReproducibilityDifference::ExitCode { .. }
        ));
    }

    #[test]
    fn differing_stdout_is_reported() {
        let a = Digest::of_bytes(b"a");
        let b = Digest::of_bytes(b"b");
        let differences = compare(&result(0, Some(a), &[]), &result(0, Some(b), &[]));
        assert_eq!(differences.len(), 1);
        assert!(matches!(
            differences[0],
            ReproducibilityDifference::Stdout { .. }
        ));
    }

    #[test]
    fn an_output_that_appears_and_disappears_is_reported() {
        let d = Digest::of_bytes(b"y");
        let differences = compare(&result(0, None, &[("out", d)]), &result(0, None, &[]));
        assert_eq!(differences.len(), 1);
        match &differences[0] {
            ReproducibilityDifference::Output {
                path,
                first,
                second,
            } => {
                assert_eq!(path, "out");
                assert_eq!(*first, Some(d));
                assert_eq!(*second, None);
            }
            other => panic!("expected Output difference, got {other:?}"),
        }
    }
}
