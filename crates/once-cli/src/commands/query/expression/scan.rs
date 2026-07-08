//! String scanning utilities shared by the parser. They understand
//! Cypher's quoting and delimiter nesting so keyword and separator
//! searches never trip over string literals or bracketed sub-expressions.

use anyhow::{bail, Result};

pub(super) fn strip_trailing_semicolon(raw: &str) -> &str {
    raw.strip_suffix(';').map_or(raw, str::trim_end)
}

pub(super) fn keyword_pos(input: &str, keyword: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices().skip_while(|(idx, _)| *idx < start) {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if input[idx..]
            .get(..keyword.len())
            .is_some_and(|part| part.eq_ignore_ascii_case(keyword))
            && keyword_boundary(input, idx, keyword.len())
        {
            return Some(idx);
        }
    }
    None
}

pub(super) fn starts_with_keyword(input: &str, keyword: &str) -> bool {
    input
        .get(..keyword.len())
        .is_some_and(|part| part.eq_ignore_ascii_case(keyword))
        && keyword_boundary(input, 0, keyword.len())
}

fn keyword_boundary(input: &str, start: usize, len: usize) -> bool {
    let before = input[..start].chars().next_back();
    let after = input[start + len..].chars().next();
    before.is_none_or(|ch| !is_identifier_char(ch))
        && after.is_none_or(|ch| !is_identifier_char(ch))
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

pub(super) fn split_top_level(input: &str, separator: char) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ch if ch == separator && depth == 0 => {
                parts.push(input[start..idx].trim());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
        if depth < 0 {
            bail!("unbalanced delimiters");
        }
    }
    if quote.is_some() {
        bail!("unterminated string literal");
    }
    if depth != 0 {
        bail!("unbalanced delimiters");
    }
    parts.push(input[start..].trim());
    Ok(parts)
}

pub(super) fn split_top_level_keyword<'a>(input: &'a str, keyword: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut start = 0;
    while let Some(pos) = keyword_pos(input, keyword, start) {
        parts.push(input[start..pos].trim());
        start = pos + keyword.len();
    }
    parts.push(input[start..].trim());
    parts
}

pub(super) fn top_level_char(input: &str, needle: char) -> Option<usize> {
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ch if ch == needle && depth == 0 => return Some(idx),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

pub(super) fn matching_delimiter(
    input: &str,
    open: usize,
    left: char,
    right: char,
) -> Result<usize> {
    if !input[open..].starts_with(left) {
        bail!("expected `{left}`");
    }
    let mut depth = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices().skip_while(|(idx, _)| *idx < open) {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ch if ch == left => depth += 1,
            ch if ch == right => {
                depth -= 1;
                if depth == 0 {
                    return Ok(idx);
                }
            }
            _ => {}
        }
    }
    bail!("unclosed `{left}`");
}
