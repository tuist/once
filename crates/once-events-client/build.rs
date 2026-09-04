fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Point tonic-build/prost-build at a vendored protoc so the build
    // works without a system-installed protobuf compiler (CI runners
    // typically don't have one).
    if std::env::var_os("PROTOC").is_none() {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }
    let proto = "proto/once/events/v1/events.proto";
    println!("cargo:rerun-if-changed={proto}");
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&[proto], &["proto"])?;
    Ok(())
}
