fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "grpc-dangerous")]
    tonic_build::compile_protos("proto/trident.proto")?;
    println!("cargo::rerun-if-env-changed=TRIDENT_VERSION");
    Ok(())
}
