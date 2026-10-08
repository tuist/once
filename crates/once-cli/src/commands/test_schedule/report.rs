use std::fmt::Write as _;

use serde_json::Value;

use super::TestExecutionReport;

pub(super) fn render_human(report: &TestExecutionReport) -> String {
    let passed = report
        .runs
        .iter()
        .filter(|run| run.get("success").and_then(Value::as_bool) == Some(true))
        .count();
    let mut output = format!(
        "once: ran {} test batches across {} local workers, {} passed, {} failed, {} ms\nplan: {}\nnext plan: {} ({} batches)\nschedule: {}\n",
        report.runs.len(), report.schedule.workers, passed, report.runs.len().saturating_sub(passed),
        report.schedule.duration_ms, report.plan.id, report.next_plan.id, report.next_plan.batches.len(), report.schedule.id,
    );
    for run in &report.runs {
        let target = run["target"].as_str().unwrap_or("unknown");
        if let Some(summary) = run.pointer("/results/summary") {
            let _ = writeln!(
                output,
                "  {target}: {} passed, {} failed, {} skipped ({} total)",
                summary["passed"], summary["failed"], summary["skipped"], summary["total"]
            );
        }
        if run["success"].as_bool() != Some(true) {
            if let Some(logs) = run
                .pointer("/results/artifacts/logs")
                .and_then(Value::as_array)
            {
                for log in logs.iter().filter_map(Value::as_str) {
                    let _ = writeln!(output, "  log: {log}");
                }
            } else if let Some(error) = run["stderr"].as_str() {
                for line in error
                    .lines()
                    .rev()
                    .take(12)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    let _ = writeln!(output, "  {line}");
                }
            }
        }
    }
    output
}
