use super::*;

#[test]
fn diagnostic_urls_strip_credentials_queries_and_fragments() {
    for (url, expected) in [
        (
            "https://user:password@example.invalid/team/private.git?token=query-secret#fragment-secret",
            "https://example.invalid/team/private.git",
        ),
        (
            "https://token%40secret@example.invalid/team/private.git#fragment-secret?query-secret",
            "https://example.invalid/team/private.git",
        ),
        (
            "ssh://user:password@[::1]:2222/team/private.git",
            "ssh://[::1]:2222/team/private.git",
        ),
        (
            "git@example.invalid:team/private.git?query-secret#fragment-secret",
            "example.invalid:team/private.git",
        ),
        (
            "example.invalid:team/private.git",
            "example.invalid:team/private.git",
        ),
        (
            "/local/team@work/private.git",
            "/local/team@work/private.git",
        ),
        ("file:///local/private.git", "file:///local/private.git"),
        (
            "https://example.invalid/team/@package.git",
            "https://example.invalid/team/@package.git",
        ),
        (
            "https://user:password@example.invalid",
            "https://example.invalid",
        ),
        (
            "https://example.invalid/line\nbreak.git",
            "https://example.invalid/line\\nbreak.git",
        ),
    ] {
        assert_eq!(diagnostic_url(url), expected);
    }
}

#[test]
fn validation_errors_identify_the_checkout_without_exposing_credentials() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("checkouts/private-package");
    let error = checkout(
        "unused-git",
        "https://user:password@example.invalid/private.git?token=secret#fragment",
        "invalid",
        &path,
        temporary.path(),
    )
    .unwrap_err();
    let error = format!("{error:#}");
    assert!(
        error.contains("https://example.invalid/private.git"),
        "{error}"
    );
    assert!(error.contains("checkouts/private-package"), "{error}");
    assert!(
        error.contains("full hexadecimal commit revision"),
        "{error}"
    );
    for secret in ["user", "password", "token", "secret", "fragment"] {
        assert!(!error.contains(secret), "{error}");
    }
}
