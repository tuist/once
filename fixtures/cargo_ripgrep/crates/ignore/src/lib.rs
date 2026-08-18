use std::path::Path;

pub fn is_ignored(patterns: &str, candidate: &str) -> bool {
    patterns.lines().any(|line| line.trim() == candidate)
}

pub fn read_patterns(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}
