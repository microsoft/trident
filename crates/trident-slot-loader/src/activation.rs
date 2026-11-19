use std::{
    fs,
    io::{self, Write},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use log::{info, warn};

use crate::{
    config::{self, ConfextMode, Config, Slot},
    manifest::Manifest,
    release,
    tools::Tools,
};

const PENDING: &str = "slot-loader.pending";

struct Link {
    path: PathBuf,
    target: PathBuf,
    exists: bool,
}

pub(super) fn activate(
    config: &Config,
    slot: Slot,
    manifest: &Manifest,
    mode: ConfextMode,
    tools: &impl Tools,
) -> Result<(), Error> {
    if manifest.sysexts.is_empty() && manifest.confexts.is_empty() {
        return Ok(());
    }
    let slot_dir = config.slot_dir(slot)?;
    for (kind, entries) in [
        ("sysexts", &manifest.sysexts),
        ("confexts", &manifest.confexts),
    ] {
        for entry in entries {
            config::regular_file(&slot_dir.join(kind).join(format!("{}.raw", entry.name)))?;
        }
    }
    let mut links = Vec::new();
    for (kind, target_dir, entries) in [
        ("sysexts", "extensions", manifest.sysexts.as_slice()),
        (
            "confexts",
            "confexts",
            if mode == ConfextMode::Systemd {
                manifest.confexts.as_slice()
            } else {
                &[]
            },
        ),
    ] {
        for entry in entries {
            let filename = format!("{}.raw", entry.name);
            links.push(Link {
                path: config.run.join(target_dir).join(&filename),
                target: config
                    .host_store
                    .join(slot.name())
                    .join(kind)
                    .join(filename),
                exists: false,
            });
        }
    }
    for link in &mut links {
        if let Some(parent) = link.path.parent().filter(|path| path.exists()) {
            config::directory(parent)?;
        }
        match fs::symlink_metadata(&link.path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_symlink() && fs::read_link(&link.path)? == link.target,
                    "refusing to overwrite {}",
                    link.path.display()
                );
                link.exists = true;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", link.path.display()))
            }
        }
    }

    let mut mounts = Vec::new();
    let mut created_links = Vec::new();
    let result = (|| {
        let mut lowerdirs = Vec::new();
        if mode == ConfextMode::Overlay && !manifest.confexts.is_empty() {
            let host = release::host(&config.root)?;
            let mount_parent = config.run.join("trident-slot-loader/etc").join(slot.name());
            config::create_directory(&mount_parent)?;
            for entry in &manifest.confexts {
                let mount = mount_parent.join(&entry.name);
                fs::create_dir(&mount)
                    .with_context(|| format!("creating mountpoint {}", mount.display()))?;
                let image = slot_dir
                    .join("confexts")
                    .join(format!("{}.raw", entry.name));
                info!("slot={} mount={}", slot.name(), image.display());
                mounts.push(mount.clone());
                tools.mount(&image, &mount)?;
                let lower = release::validate_confext(&mount, &entry.name, &host)?;
                ensure!(
                    !lower
                        .as_os_str()
                        .as_encoded_bytes()
                        .iter()
                        .any(|byte| matches!(byte, b':' | b',' | b'\\' | b'\n')),
                    "invalid overlay lowerdir {}",
                    lower.display()
                );
                lowerdirs.push(lower);
            }
        }
        for link in links.iter().filter(|link| !link.exists) {
            let parent = link
                .path
                .parent()
                .expect("invariant: runtime link has a parent");
            config::create_directory(parent)?;
            symlink(&link.target, &link.path)
                .with_context(|| format!("linking {}", link.path.display()))?;
            created_links.push(link.path.clone());
        }
        let output = config.run.join("trident-slot-loader/etc-lowerdirs");
        if !lowerdirs.is_empty() {
            publish_lowerdirs(&lowerdirs, &output)?;
        } else {
            match fs::remove_file(&output) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("removing stale lowerdir list"),
            }
        }
        Ok(())
    })();
    if result.is_err() {
        for path in created_links.iter().rev() {
            if let Err(error) = fs::remove_file(path) {
                warn!("removing link {}: {error}", path.display());
            }
        }
        for path in mounts.iter().rev() {
            if let Err(error) = tools.unmount(path) {
                warn!("unmounting {}: {error:#}", path.display());
                continue;
            }
            if let Err(error) = fs::remove_dir(path) {
                warn!("removing mountpoint {}: {error}", path.display());
            }
        }
    }
    result
}

fn publish_lowerdirs(lowerdirs: &[PathBuf], output: &Path) -> Result<(), Error> {
    let parent = output.parent().context("lowerdir output has no parent")?;
    config::create_directory(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let value = lowerdirs
        .iter()
        .map(|path| path.to_str().context("non-UTF8 lowerdir"))
        .collect::<Result<Vec<_>, _>>()?
        .join(":");
    temporary.write_all(value.as_bytes())?;
    temporary
        .persist(output)
        .with_context(|| format!("publishing {}", output.display()))?;
    Ok(())
}

pub(super) fn clear_pending(slot: &Path) -> Result<(), Error> {
    let path = slot.join(PENDING);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            config::regular_file(&path)?;
            fs::remove_file(&path).context("clearing pending marker")?;
            fs::File::open(slot)?
                .sync_all()
                .context("synchronizing pending marker removal")?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspecting pending marker"),
    }
    Ok(())
}

pub(super) fn recover(slot: &Path, enabled: bool, tools: &impl Tools) -> Result<(), Error> {
    if !enabled {
        info!("automatic update-loader reboot disabled");
        return Ok(());
    }
    let path = slot.join(PENDING);
    match fs::symlink_metadata(&path) {
        Ok(_) => config::regular_file(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info!("no pending update marker; not rebooting");
            return Ok(());
        }
        Err(error) => return Err(error).context("inspecting recovery marker"),
    }
    warn!(
        "pending update loader failed; requesting reboot, marker={}",
        path.display()
    );
    tools.reboot()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};

    use tempfile::{tempdir, TempDir};

    struct MockTools {
        mounted: RefCell<Vec<PathBuf>>,
        unmounted: RefCell<Vec<PathBuf>>,
        reboots: Cell<usize>,
        fail_name: Option<&'static str>,
    }

    impl MockTools {
        fn new() -> Self {
            Self {
                mounted: RefCell::new(Vec::new()),
                unmounted: RefCell::new(Vec::new()),
                reboots: Cell::new(0),
                fail_name: None,
            }
        }
    }

    impl Tools for MockTools {
        fn mount(&self, image: &Path, target: &Path) -> Result<(), Error> {
            let name = image.file_stem().unwrap().to_str().unwrap();
            if self.fail_name == Some(name) {
                anyhow::bail!("injected mount error");
            }
            self.mounted.borrow_mut().push(target.into());
            let release = target.join("etc/extension-release.d");
            fs::create_dir_all(&release)?;
            fs::write(
                release.join(format!("extension-release.{name}")),
                fs::read(image)?,
            )?;
            Ok(())
        }

        fn unmount(&self, target: &Path) -> Result<(), Error> {
            self.unmounted.borrow_mut().push(target.into());
            let etc = target.join("etc");
            if etc.exists() {
                fs::remove_dir_all(etc)?;
            }
            Ok(())
        }

        fn reboot(&self) -> Result<(), Error> {
            self.reboots.set(self.reboots.get() + 1);
            Ok(())
        }
    }

    fn setup() -> (TempDir, Config) {
        let temporary = tempdir().unwrap();
        let config = Config::new(
            temporary.path().join("sysroot"),
            temporary.path().join("run"),
            None,
        )
        .unwrap();
        for slot in ["a", "b"] {
            for kind in ["sysexts", "confexts"] {
                fs::create_dir_all(config.store().join(slot).join(kind)).unwrap();
            }
        }
        fs::create_dir_all(config.root.join("usr/lib")).unwrap();
        fs::write(
            config.root.join("usr/lib/os-release"),
            "ID=acl\nVERSION_ID=3.0\n",
        )
        .unwrap();
        (temporary, config)
    }

    fn manifest(config: &Config, slot: Slot, json: &str) -> Manifest {
        let path = config.slot_dir(slot).unwrap().join("slot-loader.json");
        fs::write(&path, json).unwrap();
        Manifest::read(path).unwrap()
    }

    fn image(config: &Config, slot: Slot, kind: &str, name: &str) -> PathBuf {
        let path = config
            .slot_dir(slot)
            .unwrap()
            .join(kind)
            .join(format!("{name}.raw"));
        fs::write(&path, "ID=acl\nVERSION_ID=3.0\nCONFEXT_ID=defaults\n").unwrap();
        path
    }

    #[test]
    fn native_modes_link_only_manifest_images_for_the_booted_slot() {
        for slot in [Slot::A, Slot::B] {
            let (_temporary, config) = setup();
            let tools = MockTools::new();
            image(&config, slot, "sysexts", "runtime");
            image(&config, slot, "sysexts", "unlisted");
            image(&config, slot, "confexts", "defaults");
            fs::create_dir_all(config.run.join("extensions")).unwrap();
            fs::write(config.run.join("extensions/operator.raw"), "unrelated").unwrap();
            let manifest = manifest(
                &config,
                slot,
                r#"{"version":1,"sysexts":[{"name":"runtime"}],"confexts":[{"name":"defaults"}]}"#,
            );
            let lowerdirs = config.run.join("trident-slot-loader/etc-lowerdirs");
            fs::create_dir_all(lowerdirs.parent().unwrap()).unwrap();
            fs::write(&lowerdirs, "stale").unwrap();
            activate(&config, slot, &manifest, ConfextMode::Systemd, &tools).unwrap();
            activate(&config, slot, &manifest, ConfextMode::Systemd, &tools).unwrap();
            for (native, kind, name) in [
                ("extensions", "sysexts", "runtime"),
                ("confexts", "confexts", "defaults"),
            ] {
                assert_eq!(
                    fs::read_link(config.run.join(native).join(format!("{name}.raw"))).unwrap(),
                    config
                        .host_store
                        .join(slot.name())
                        .join(kind)
                        .join(format!("{name}.raw"))
                );
            }
            assert!(!config.run.join("extensions/unlisted.raw").exists());
            assert_eq!(
                fs::read_to_string(config.run.join("extensions/operator.raw")).unwrap(),
                "unrelated"
            );
            assert!(!lowerdirs.exists());
            assert!(tools.mounted.borrow().is_empty());
        }
    }

    #[test]
    fn missing_images_symlinks_and_conflicts_fail_before_publication() {
        for failure in ["missing", "symlink", "collision"] {
            let (_temporary, config) = setup();
            let tools = MockTools::new();
            let manifest = manifest(
                &config,
                Slot::A,
                r#"{"version":1,"sysexts":[{"name":"first"},{"name":"last"}]}"#,
            );
            image(&config, Slot::A, "sysexts", "first");
            let last = config.slot_dir(Slot::A).unwrap().join("sysexts/last.raw");
            match failure {
                "symlink" => symlink("first.raw", last).unwrap(),
                "collision" => {
                    image(&config, Slot::A, "sysexts", "last");
                    fs::create_dir_all(config.run.join("extensions")).unwrap();
                    fs::write(config.run.join("extensions/last.raw"), "unrelated").unwrap();
                }
                _ => {}
            }
            activate(&config, Slot::A, &manifest, ConfextMode::Systemd, &tools).unwrap_err();
            assert!(fs::symlink_metadata(config.run.join("extensions/first.raw")).is_err());
            assert!(tools.mounted.borrow().is_empty());
        }
    }

    #[test]
    fn overlay_publishes_manifest_order_after_validation() {
        let (_temporary, config) = setup();
        let tools = MockTools::new();
        image(&config, Slot::A, "confexts", "z-high");
        image(&config, Slot::A, "confexts", "a-low");
        let manifest = manifest(
            &config,
            Slot::A,
            r#"{"version":1,"confexts":[{"name":"z-high"},{"name":"a-low"}]}"#,
        );
        activate(&config, Slot::A, &manifest, ConfextMode::Overlay, &tools).unwrap();
        let prefix = config.run.join("trident-slot-loader/etc/a");
        assert_eq!(
            fs::read_to_string(config.run.join("trident-slot-loader/etc-lowerdirs")).unwrap(),
            format!(
                "{}:{}",
                prefix.join("z-high/etc").display(),
                prefix.join("a-low/etc").display()
            )
        );
        assert!(!config.run.join("confexts").exists());
        assert_eq!(tools.mounted.borrow().len(), 2);
    }

    #[test]
    fn overlay_failures_clean_mounts_and_do_not_publish_links_or_lowerdirs() {
        for failure in ["mount", "compatibility", "publication"] {
            let (_temporary, config) = setup();
            let mut tools = MockTools::new();
            image(&config, Slot::A, "sysexts", "runtime");
            image(&config, Slot::A, "confexts", "first");
            let last = image(&config, Slot::A, "confexts", "last");
            let manifest = manifest(
                &config,
                Slot::A,
                r#"{"version":1,"sysexts":[{"name":"runtime"}],"confexts":[{"name":"first"},{"name":"last"}]}"#,
            );
            let output = config.run.join("trident-slot-loader/etc-lowerdirs");
            match failure {
                "mount" => tools.fail_name = Some("last"),
                "compatibility" => fs::write(last, "ID=wrong\nCONFEXT_ID=defaults").unwrap(),
                _ => fs::create_dir_all(&output).unwrap(),
            }
            activate(&config, Slot::A, &manifest, ConfextMode::Overlay, &tools).unwrap_err();
            assert!(fs::symlink_metadata(config.run.join("extensions/runtime.raw")).is_err());
            assert!(!output.is_file());
            assert_eq!(tools.unmounted.borrow().len(), 2);
            for mount in tools.mounted.borrow().iter() {
                assert!(!mount.exists());
            }
        }
    }

    #[test]
    fn empty_manifest_is_a_noop_but_success_can_clear_its_pending_marker() {
        let (_temporary, config) = setup();
        let tools = MockTools::new();
        let slot = config.slot_dir(Slot::A).unwrap();
        let manifest = manifest(&config, Slot::A, r#"{"version":1}"#);
        fs::write(slot.join(PENDING), "").unwrap();
        activate(&config, Slot::A, &manifest, ConfextMode::Overlay, &tools).unwrap();
        assert!(!config.run.exists());
        clear_pending(&slot).unwrap();
        assert!(!slot.join(PENDING).exists());
    }

    #[test]
    fn recovery_is_marker_gated_and_independent_of_manifest_parsing() {
        let (_temporary, config) = setup();
        let tools = MockTools::new();
        let slot = config.slot_dir(Slot::A).unwrap();
        fs::write(slot.join("slot-loader.json"), "malformed").unwrap();
        recover(&slot, true, &tools).unwrap();
        assert_eq!(tools.reboots.get(), 0);
        fs::write(slot.join(PENDING), "").unwrap();
        recover(&slot, false, &tools).unwrap();
        assert_eq!(tools.reboots.get(), 0);
        recover(&slot, true, &tools).unwrap();
        assert_eq!(tools.reboots.get(), 1);
        assert!(slot.join(PENDING).exists());
        fs::remove_file(slot.join(PENDING)).unwrap();
        symlink("slot-loader.json", slot.join(PENDING)).unwrap();
        recover(&slot, true, &tools).unwrap_err();
        assert_eq!(tools.reboots.get(), 1);
    }
}
