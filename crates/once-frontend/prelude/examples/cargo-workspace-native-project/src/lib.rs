pub fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

#[cfg(test)]
mod tests {
    #[test]
    fn greets_a_name() {
        assert_eq!(super::greeting("Once"), "Hello, Once!");
    }
}
