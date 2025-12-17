fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=TRIDENT_VERSION");
    Ok(())
}
