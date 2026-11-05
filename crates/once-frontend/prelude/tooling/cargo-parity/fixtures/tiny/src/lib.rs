mod selected;

pub fn value() -> u32 {
    selected::VALUE + if cfg!(feature = "extra") { 11 } else { 0 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn configured_value() {
        assert_eq!(super::value(), 18);
        assert!(cfg!(debug_assertions));
    }

    #[test]
    #[ignore]
    fn explicitly_ignored() {
        panic!("must remain ignored");
    }
}
