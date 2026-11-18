pub fn generate(schema: &str) -> u32 {
    schema.trim().parse().unwrap()
}
