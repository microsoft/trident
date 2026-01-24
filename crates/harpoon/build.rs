fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Re-run the build script if any of the proto files change (scan the proto directory)
    for entry in std::fs::read_dir("../../proto")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("proto") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    tonic_prost_build::compile_protos("../../proto/harpoon/v1/harpoon.proto")?;
    Ok(())
}
