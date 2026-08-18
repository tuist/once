pub fn render(line: &str) -> String {
    let mut buffer = itoa::Buffer::new();
    format!("{} {line}", buffer.format(line.len()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_a_line_with_its_length() {
        assert_eq!(super::render("abc"), "3 abc");
    }
}
