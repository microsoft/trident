use std::{fs, path::Path};

use anyhow::{Context, Error};
use log::info;
use tera::{Context as TeraCtx, Tera};

pub fn generate(dir: &Path) -> Result<(), Error> {
    // Read the trident crate version from its Cargo.toml
    let cargo_toml_path = dir.join("../../crates/trident/Cargo.toml");
    let cargo_toml =
        fs::read_to_string(&cargo_toml_path).context("Failed to read trident Cargo.toml")?;
    let cargo_version = cargo_toml
        .parse::<toml::Table>()
        .context("Failed to parse trident Cargo.toml")?["package"]["version"]
        .as_str()
        .context("Missing version in trident Cargo.toml")?
        .to_string();
    info!("Trident crate version: {cargo_version}");

    let template_path = dir.join("template.spec");
    let template_str =
        fs::read_to_string(&template_path).context("Failed to read template.spec")?;

    let mut tera = Tera::default();
    tera.add_raw_template("template.spec", &template_str)
        .context("Failed to parse template.spec")?;

    let header = indoc::indoc! {"
        # AUTO-GENERATED FILE — DO NOT EDIT
        # Edit packaging/rpm/template.spec instead, then run: make generate-specs
    "};

    // Generate trident.spec (local/repo build)
    let mut ctx = TeraCtx::new();
    ctx.insert("azl_repo", &false);
    ctx.insert("cargo_version", &cargo_version);
    let rendered = tera
        .render("template.spec", &ctx)
        .context("Failed to render trident.spec")?;
    let out = dir.join("trident.spec");
    fs::write(&out, format!("{header}{rendered}")).context("Failed to write trident.spec")?;
    info!("Generated {}", out.display());

    // Generate trident-azl.spec (Azure Linux distro build)
    let mut ctx = TeraCtx::new();
    ctx.insert("azl_repo", &true);
    ctx.insert("cargo_version", &cargo_version);
    let rendered = tera
        .render("template.spec", &ctx)
        .context("Failed to render trident-azl.spec")?;
    let out = dir.join("trident-azl.spec");
    fs::write(&out, format!("{header}{rendered}")).context("Failed to write trident-azl.spec")?;
    info!("Generated {}", out.display());

    Ok(())
}
