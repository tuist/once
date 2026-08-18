// The path is relative, so this only resolves when the test process runs from
// its own package root the way Cargo runs it.
const PATTERNS: &str = "tests/gitignore.patterns";

#[test]
fn reads_patterns_relative_to_the_package_root() {
    let patterns = ignore::read_patterns(std::path::Path::new(PATTERNS))
        .expect("failed to open the patterns file");
    assert!(ignore::is_ignored(&patterns, "target"));
}
