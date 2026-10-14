//! Compiles `proto/plasmaslate/v1/slate.proto` with `protox` (pure Rust, no `protoc` needed)
//! and generates Rust types with `prost-build`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../proto");
    let file = proto_root.join("plasmaslate/v1/slate.proto");
    println!("cargo:rerun-if-changed={}", file.display());

    let fds = protox::compile([&file], [&proto_root])?;
    prost_build::Config::new().compile_fds(fds)?;
    Ok(())
}
