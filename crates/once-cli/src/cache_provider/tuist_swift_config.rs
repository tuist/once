//! Extracts the Tuist project handle and cache URL from a `Tuist.swift`
//! manifest. Swift has no cheap off-the-shelf parser here, so this is a
//! narrow scanner: it walks the source while skipping comments and string
//! literals, finds the top-level `Tuist(...)` constructor, and reads the
//! `fullHandle` and `url` string arguments out of its argument list.

/// Reads `(fullHandle, url)` from the top-level `Tuist(...)` constructor.
/// Returns `None` when the manifest has no such constructor or no
/// non-empty `fullHandle`.
pub(super) fn parse_tuist_config(src: &str) -> Option<(String, Option<String>)> {
    let body = tuist_constructor_body(src)?;
    let full_handle = string_argument(body, "fullHandle")?;
    let url = string_argument(body, "url");
    Some((full_handle, url))
}

fn tuist_constructor_body(src: &str) -> Option<&str> {
    let mut cursor = 0;
    while cursor < src.len() {
        if let Some(next) = skip_comment_or_string(src, cursor) {
            cursor = next;
            continue;
        }
        if starts_identifier(src, cursor, "Tuist") {
            let open = skip_space_and_comments(src, cursor + "Tuist".len())?;
            if src[open..].starts_with('(') {
                return parenthesized_body(src, open);
            }
        }
        cursor += src[cursor..].chars().next()?.len_utf8();
    }
    None
}

fn parenthesized_body(src: &str, open: usize) -> Option<&str> {
    let mut cursor = open + 1;
    let mut depth = 1;
    while cursor < src.len() {
        if let Some(next) = skip_comment_or_string(src, cursor) {
            cursor = next;
            continue;
        }
        let ch = src[cursor..].chars().next()?;
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(&src[open + 1..cursor]);
            }
        }
        cursor += ch.len_utf8();
    }
    None
}

fn string_argument(src: &str, label: &str) -> Option<String> {
    let mut cursor = 0;
    let mut depth = 0usize;
    while cursor < src.len() {
        if let Some(next) = skip_comment_or_string(src, cursor) {
            cursor = next;
            continue;
        }
        let ch = src[cursor..].chars().next()?;
        if matches!(ch, '(' | '[' | '{') {
            depth += 1;
            cursor += ch.len_utf8();
            continue;
        }
        if matches!(ch, ')' | ']' | '}') {
            depth = depth.saturating_sub(1);
            cursor += ch.len_utf8();
            continue;
        }
        if depth == 0 && starts_identifier(src, cursor, label) {
            let colon = skip_space_and_comments(src, cursor + label.len())?;
            if src[colon..].starts_with(':') {
                return leading_string(&src[colon + 1..]);
            }
        }
        cursor += ch.len_utf8();
    }
    None
}

fn starts_identifier(src: &str, index: usize, identifier: &str) -> bool {
    if !src[index..].starts_with(identifier) {
        return false;
    }
    let before = src[..index].chars().next_back();
    let after = src[index + identifier.len()..].chars().next();
    !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn skip_space_and_comments(src: &str, mut cursor: usize) -> Option<usize> {
    loop {
        while cursor < src.len() {
            let ch = src[cursor..].chars().next()?;
            if !ch.is_whitespace() {
                break;
            }
            cursor += ch.len_utf8();
        }
        if let Some(next) = skip_comment(src, cursor) {
            cursor = next;
        } else {
            return Some(cursor);
        }
    }
}

fn skip_comment_or_string(src: &str, cursor: usize) -> Option<usize> {
    skip_comment(src, cursor).or_else(|| skip_string(src, cursor))
}

fn skip_comment(src: &str, cursor: usize) -> Option<usize> {
    if src[cursor..].starts_with("//") {
        return Some(
            src[cursor..]
                .find('\n')
                .map_or(src.len(), |offset| cursor + offset + 1),
        );
    }
    if !src[cursor..].starts_with("/*") {
        return None;
    }
    let mut depth = 1usize;
    let mut current = cursor + 2;
    while current < src.len() {
        if src[current..].starts_with("/*") {
            depth += 1;
            current += 2;
        } else if src[current..].starts_with("*/") {
            depth -= 1;
            current += 2;
            if depth == 0 {
                return Some(current);
            }
        } else {
            current += src[current..].chars().next()?.len_utf8();
        }
    }
    Some(src.len())
}

fn skip_string(src: &str, cursor: usize) -> Option<usize> {
    if !src[cursor..].starts_with('"') {
        return None;
    }
    if src[cursor..].starts_with("\"\"\"") {
        return Some(
            src[cursor + 3..]
                .find("\"\"\"")
                .map_or(src.len(), |offset| cursor + 3 + offset + 3),
        );
    }
    let mut current = cursor + 1;
    let mut escaped = false;
    while current < src.len() {
        let ch = src[current..].chars().next()?;
        current += ch.len_utf8();
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(current);
        }
    }
    Some(src.len())
}

fn leading_string(src: &str) -> Option<String> {
    let trimmed = src.trim_start();
    let body = trimmed.strip_prefix('"')?;
    let mut value = String::new();
    let mut escaped = false;
    for ch in body.chars() {
        if escaped {
            value.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return non_empty(&value);
        } else {
            value.push(ch);
        }
    }
    None
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
