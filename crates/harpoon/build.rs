fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Re-run the build script if any of the proto files change (scan the proto directory)
    println!("cargo:rerun-if-changed=../../proto/harpoon/v1/harpoon.proto");
    tonic_prost_build::compile_protos("../../proto/harpoon/v1/harpoon.proto")?;
    Ok(())
}
