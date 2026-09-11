fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=TRIDENT_VERSION");
    println!("cargo:rerun-if-env-changed=AZURE_MONITOR_CONNECTION_STRING");
    Ok(())
}
