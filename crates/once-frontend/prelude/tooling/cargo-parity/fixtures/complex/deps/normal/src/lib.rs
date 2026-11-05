pub fn value() -> u32 {
    5
}

#[cfg(test)]
mod tests {
    #[test]
    fn nondefault_member_scope() {
        assert_eq!(super::value(), 5);
    }
}
