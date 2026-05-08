pub fn app_bundle_path(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}.app")
    } else {
        format!(".fabrik/out/{package}/{name}.app")
    }
}
