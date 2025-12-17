fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../proto/harpoon.proto");
    tonic_prost_build::compile_protos("../../proto/harpoon.proto")?;
    Ok(())
}
