pub(crate) fn captured_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    console::strip_ansi_codes(&text)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_output_cannot_replay_terminal_controls() {
        let text =
            b"\x1b]7501;state=error:app=child\x1b\\\x1b[?2026h\x1b[31mfailed\x1b[0m\x1b[?2026l";
        assert_eq!(captured_text(text), "failed");
        assert_eq!(
            captured_text(b"partial \x1b]7501;state=working"),
            "partial "
        );
        assert_eq!(
            captured_text("result\u{009c}\u{009b}\n\té".as_bytes()),
            "result\n\té"
        );
    }
}
