fn main() {
    println!("{}", find(&std::env::args().nth(1).unwrap_or_default()));
}

pub fn find(name: &str) -> String {
    if name.is_empty() { "nothing".to_string() } else { format!("found {name}") }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_nothing_without_a_name() {
        assert_eq!(super::find(""), "nothing");
    }
}
