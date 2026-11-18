use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use struo_cas::Digest;
use struo_core::ReproducibilityReport;

use super::VerificationReport;

const MAX_ENTRIES: usize = 10_000;
const MAX_DIFFERENCES: usize = 100;

#[derive(Debug, Serialize, Deserialize)]
pub struct ArtifactDifference {
    pub path: String,
    pub kind: String,
    pub log: bool,
    pub first_differing_byte: Option<u64>,
    pub first_excerpt: Option<String>,
    pub second_excerpt: Option<String>,
}

#[derive(PartialEq, Eq)]
struct Entry {
    kind: &'static str,
    mode: u32,
    content: Option<Digest>,
}

fn inventory(root: &Path) -> Result<(BTreeMap<String, Entry>, bool)> {
    let mut entries = BTreeMap::new();
    if !root.exists() {
        return Ok((entries, false));
    }
    for (index, entry) in walkdir::WalkDir::new(root)
        .min_depth(1)
        .max_open(MAX_ENTRIES + 1)
        .into_iter()
        .enumerate()
    {
        if index == MAX_ENTRIES {
            return Ok((entries, true));
        }
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777
        };
        #[cfg(not(unix))]
        let mode = u32::from(metadata.permissions().readonly());
        let (kind, content) = if metadata.file_type().is_symlink() {
            (
                "symlink",
                Some(Digest::of_bytes(
                    std::fs::read_link(entry.path())?
                        .to_string_lossy()
                        .as_bytes(),
                )),
            )
        } else if metadata.is_dir() {
            ("directory", None)
        } else {
            (
                "file",
                Some(Digest::of_reader(std::fs::File::open(entry.path())?)?),
            )
        };
        entries.insert(
            entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/"),
            Entry {
                kind,
                mode,
                content,
            },
        );
    }
    Ok((entries, false))
}

pub(super) fn compare(
    first_root: &Path,
    second_root: &Path,
    comparison: &mut ReproducibilityReport,
) -> Result<(Vec<ArtifactDifference>, bool)> {
    let log_paths = comparison
        .results
        .iter()
        .map(|trial| trial.log_paths.iter().cloned().collect::<BTreeSet<_>>())
        .reduce(|first, second| first.intersection(&second).cloned().collect())
        .unwrap_or_default();
    let has_artifacts = comparison
        .results
        .iter()
        .filter_map(|trial| trial.result.as_ref())
        .any(|result| result.outputs.keys().any(|path| !log_paths.contains(path)));
    let (first, first_truncated) = inventory(first_root)?;
    let (second, second_truncated) = inventory(second_root)?;
    let paths = first.keys().chain(second.keys()).collect::<BTreeSet<_>>();
    let mut differences = Vec::new();
    let mut truncated = first_truncated || second_truncated;
    for path in paths {
        let a = first.get(path);
        let b = second.get(path);
        if a == b {
            continue;
        }
        if differences.len() == MAX_DIFFERENCES {
            truncated = true;
            break;
        }
        let kind = match (a, b) {
            (None, _) => "added",
            (_, None) => "removed",
            (Some(a), Some(b)) if a.kind != b.kind => "type",
            (Some(a), Some(b)) if a.content == b.content => "permissions",
            (Some(a), Some(_)) if a.kind == "symlink" => "symlink_target",
            _ => "content",
        };
        let mut difference = ArtifactDifference {
            path: path.clone(),
            kind: kind.into(),
            log: log_paths.contains(path),
            first_differing_byte: None,
            first_excerpt: None,
            second_excerpt: None,
        };
        if kind == "content" {
            let first_path = first_root.join(path);
            let second_path = second_root.join(path);
            difference.first_differing_byte = first_difference(&first_path, &second_path)?;
            let offset = difference
                .first_differing_byte
                .unwrap_or(0)
                .saturating_sub(80);
            difference.first_excerpt = excerpt(&first_path, offset)?;
            difference.second_excerpt = excerpt(&second_path, offset)?;
        }
        differences.push(difference);
    }
    if truncated && !log_paths.is_empty() && comparison.artifacts_match == Some(false) {
        comparison.artifacts_match = None;
        comparison.logs_match = None;
    } else if differences.iter().any(|difference| difference.log) {
        comparison.logs_match = Some(false);
        if comparison.execution_succeeded && !truncated {
            comparison.artifacts_match =
                Some(!differences.iter().any(|difference| !difference.log));
        }
    }
    if !has_artifacts {
        comparison.artifacts_match = None;
    }
    Ok((differences, truncated))
}

fn first_difference(first: &Path, second: &Path) -> Result<Option<u64>> {
    let mut a = std::io::BufReader::new(std::fs::File::open(first)?);
    let mut b = std::io::BufReader::new(std::fs::File::open(second)?);
    let mut offset = 0;
    loop {
        let mut left = Vec::with_capacity(8192);
        let mut right = Vec::with_capacity(8192);
        let left_len = a.by_ref().take(8192).read_to_end(&mut left)?;
        let right_len = b.by_ref().take(8192).read_to_end(&mut right)?;
        if let Some(index) = left.iter().zip(&right).position(|(a, b)| a != b) {
            return Ok(Some(offset + index as u64));
        }
        if left_len != right_len {
            return Ok(Some(offset + left_len.min(right_len) as u64));
        }
        if left_len == 0 {
            return Ok(None);
        }
        offset += left_len as u64;
    }
}

fn excerpt(path: &Path, offset: u64) -> Result<Option<String>> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(240).read_to_end(&mut bytes)?;
    Ok((!bytes
        .iter()
        .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r' | b'\t')))
    .then(|| String::from_utf8_lossy(&bytes).into_owned()))
}

pub(super) fn render(report: &VerificationReport) -> String {
    let outcome = if report.success { "passed" } else { "failed" };
    let mut text = format!("verification {} {outcome}: {}\n", report.id, report.subject);
    for (index, trial) in report.comparison.results.iter().enumerate() {
        let status = trial.result.as_ref().map_or_else(
            || "not completed".into(),
            |result| format!("exit {}", result.exit_code),
        );
        let _ = writeln!(text, "  trial {}: {status}", index + 1);
        if let Some(error) = &trial.error {
            let _ = writeln!(text, "    {error}");
        }
    }
    if report.comparison.results.windows(2).any(|pair| {
        pair[0].action_digest.is_some()
            && pair[1].action_digest.is_some()
            && pair[0].action_digest != pair[1].action_digest
    }) {
        text.push_str("  Declared action identities differ between trials; inspect their results and dependencies.\n");
    }
    let artifacts = match report.comparison.artifacts_match {
        Some(true) => "no divergence observed",
        Some(false) => "DIFFER",
        None if report.differences_truncated && report.comparison.execution_succeeded => {
            "not fully compared (detail limit)"
        }
        None if report.comparison.execution_succeeded => "not compared (no declared outputs)",
        None => "not compared (execution incomplete or failed)",
    };
    let logs = match report.comparison.logs_match {
        Some(true) => "no divergence observed",
        Some(false) => "differ",
        None => "not compared",
    };
    let _ = writeln!(
        text,
        "  artifacts: {artifacts}\n  logs: {logs}\n  scope: {}",
        report.scope
    );
    for difference in &report.artifact_differences {
        let role = if difference.log { "log" } else { "artifact" };
        let _ = writeln!(text, "  {role} {}: {}", difference.path, difference.kind);
        if let Some(offset) = difference.first_differing_byte {
            let _ = writeln!(text, "    first differing byte: {offset}");
        }
        if let (Some(first), Some(second)) = (&difference.first_excerpt, &difference.second_excerpt)
        {
            let _ = writeln!(
                text,
                "    trial 1 excerpt: {first:?}\n    trial 2 excerpt: {second:?}"
            );
        }
    }
    if report.differences_truncated {
        text.push_str("  Detail listing truncated; full artifacts are retained.\n");
    }
    for directory in &report.trial_directories {
        let _ = writeln!(text, "  retained: {directory}");
    }
    for limitation in &report.limitations {
        let _ = writeln!(text, "  limitation: {limitation}");
    }
    let _ = write!(text, "Inspect again: {}", report.inspect_command);
    text
}
