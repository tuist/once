pub fn describe(pattern: &str) -> String {
    format!("searching for {pattern}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn describes_a_pattern() {
        assert_eq!(super::describe("needle"), "searching for needle");
    }
}
