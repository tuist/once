pub fn cache_name() -> &'static str {
    "cache"
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_the_cache() {
        assert_eq!(super::cache_name(), "cache");
    }
}
