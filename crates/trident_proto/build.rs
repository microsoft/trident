use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

fn main() -> Result<(), Box<dyn Error>> {
    let include_dir = PathBuf::from("../../proto");
    let mut proto_files = Vec::new();
    add_protos(&mut proto_files, "../../proto/trident/v1")?;
    add_protos(&mut proto_files, "../../proto/trident/v1preview")?;

    tonic_prost_build::configure()
        .server_mod_attribute(".", "#[cfg(feature = \"server\")]")
        .compile_protos(&proto_files, &[include_dir])?;

    Ok(())
}

// Compiles all prod protos in ../../proto/trident/v1.
fn add_protos(
    protos: &mut Vec<PathBuf>,
    proto_dir: impl AsRef<Path>,
) -> Result<(), Box<dyn Error>> {
    let new_protos: Vec<_> = fs::read_dir(proto_dir.as_ref())?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().is_some_and(|ext| ext == "proto") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    println!("cargo:rerun-if-changed={}", proto_dir.as_ref().display());
    for proto in &new_protos {
        println!("cargo:rerun-if-changed={}", proto.display());
    }

    protos.extend(new_protos);

    Ok(())
}
