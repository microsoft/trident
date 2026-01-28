fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=TRIDENT_VERSION");
    println!("cargo:rerun-if-changed=../../proto/harpoon.proto");

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true) // Include server for testing
        .compile_protos(&["../../proto/harpoon.proto"], &["../../proto/"])?;

    Ok(())
}
