fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("../../proto/harpoon.proto")?;
    println!("cargo:rerun-if-env-changed=TRIDENT_VERSION");
    Ok(())
}
