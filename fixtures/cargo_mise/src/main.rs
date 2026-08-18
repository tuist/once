include!(concat!(env!("OUT_DIR"), "/registry.rs"));

fn main() {
    println!("{} {}", REGISTRY, mise_cache_core::cache_name());
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_registry_is_available() {
        assert_eq!(super::REGISTRY, "aqua");
    }
}
