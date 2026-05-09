fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let flag = std::fs::read_to_string(format!("{manifest_dir}/flag.txt")).unwrap();
    if flag.trim() == "enabled" {
        println!("cargo::rustc-cfg=build_script_enabled");
    }
}
