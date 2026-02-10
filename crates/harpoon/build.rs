use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Re-run the build script if the proto file changes
    let proto_path_prod = "../../proto/harpoon/v1/harpoon.proto";
    let proto_path_prev = "../../proto/harpoon/v1/harpoon_preview.proto";
    compile_protos(proto_path_prod)?;
    compile_protos(proto_path_prev)?;

    Ok(())
}

fn compile_protos(proto_path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed={}", proto_path.as_ref().display());
    tonic_prost_build::compile_protos(proto_path)?;

    Ok(())
}
