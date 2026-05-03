cargo_binary(
    name = "fabrik",
    cargo_package = "fabrik-cli",
    bin = "fabrik",
    srcs = [
        "Cargo.lock",
        "Cargo.toml",
    ] + glob([
        "crates/*/Cargo.toml",
        "crates/*/prelude/**/*.star",
        "crates/*/src/**/*.rs",
    ]),
)
