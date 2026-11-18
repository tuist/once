include!(concat!(env!("OUT_DIR"), "/generated.rs"));
pub const CONFIGURATION: &str = include_str!(concat!(env!("OUT_DIR"), "/configuration.txt"));

pub fn value() -> u32 {
    normal::value() + GENERATED + if cfg!(feature = "extra") { 11 } else { 0 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_contract() {
        assert_eq!(super::value(), 39);
        assert!(cfg!(generated_input));
        assert_eq!(env!("E7_MARKER"), "generated");
        assert!(super::CONFIGURATION.contains("PROFILE=debug\n"));
        assert!(checker::check(super::value()));
    }
}
