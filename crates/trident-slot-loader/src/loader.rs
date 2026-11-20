//! Activation orchestration and the runtime transaction boundary.
//!
//! Planning/preflight precede all mutations. Validated mounts precede native links; the lowerdir
//! handoff is published last. Until that succeeds, only this invocation's resources are rolled back.
//! Successful mounts intentionally outlive the process so Bootengine can consume their paths.

use anyhow::Error;
use log::{error, info};

use crate::{
    config::{ConfextMode, Paths},
    diagnostics, filesystem,
    manifest::Manifest,
    mounts::MountSet,
    outputs::LowerdirOutput,
    recovery,
    selection::{Plan, Slot},
    symlinks::LinkSet,
    tools::ImageMounter,
};

pub(super) fn run(
    paths: &Paths,
    slot: Slot,
    mode: ConfextMode,
    mounter: &impl ImageMounter,
) -> Result<(), Error> {
    let slot_dir = paths.store().join(slot.name());
    filesystem::directory(&slot_dir)?;
    let manifest = diagnostics::stage("manifest", || {
        Manifest::read(slot_dir.join("slot-loader.json"))
    })?;
    let plan = diagnostics::stage("selection", || Plan::build(paths, slot, &manifest, mode))?;
    info!(
        "event=plan_ready slot={slot} confext_mode={mode} symlink_count={} mount_count={}",
        plan.links.len(),
        plan.mounts.len()
    );
    activate(paths, plan, mounter)?;
    // Publication is already committed to /run. Marker failure still fails the unit, but must not
    // tear down published lowerdirs behind a consumer; root setup must require unit success.
    diagnostics::stage("pending_marker", || recovery::clear_pending(&slot_dir))?;
    info!("event=loader_ready slot={slot} native_merge=not_performed");
    Ok(())
}

fn activate(paths: &Paths, plan: Plan, mounter: &impl ImageMounter) -> Result<(), Error> {
    if plan.is_empty() {
        info!("event=activation_skipped reason=empty_manifest");
        return Ok(());
    }
    let (links, output) = diagnostics::stage("preflight", || {
        Ok((
            LinkSet::preflight(plan.links)?,
            LowerdirOutput::preflight(paths.lowerdirs())?,
        ))
    })?;
    let mut transaction = Transaction {
        links,
        mounts: MountSet::new(mounter),
        armed: true,
    };
    let result = (|| {
        let lowerdirs = diagnostics::stage("confext_mounts", || {
            transaction.mounts.prepare(&plan.mounts, paths.sysroot())
        })?;
        diagnostics::stage("native_links", || transaction.links.publish())?;
        diagnostics::stage("handoff", || output.publish(&lowerdirs))
    })();
    match result {
        Ok(()) => {
            transaction.armed = false;
            Ok(())
        }
        Err(error) => match transaction.rollback() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!("runtime rollback incomplete: {cleanup:#}"))),
        },
    }
}

struct Transaction<'a, M: ImageMounter> {
    links: LinkSet,
    mounts: MountSet<'a, M>,
    armed: bool,
}

impl<M: ImageMounter> Transaction<'_, M> {
    fn rollback(&mut self) -> Result<(), Error> {
        self.armed = false;
        diagnostics::stage("cleanup", || {
            let errors: Vec<_> = self
                .links
                .rollback()
                .into_iter()
                .chain(self.mounts.rollback())
                .collect();
            for error in &errors {
                error!("event=cleanup_failed error={error:#}");
            }
            if !errors.is_empty() {
                anyhow::bail!(
                    "{}",
                    errors
                        .iter()
                        .map(|error| format!("{error:#}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
            Ok(())
        })
    }
}

impl<M: ImageMounter> Drop for Transaction<'_, M> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.rollback() {
                error!("event=unwind_cleanup_failed error={error:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        cell::RefCell,
        fs,
        os::unix::fs::symlink,
        path::{Path, PathBuf},
    };

    use tempfile::{tempdir, TempDir};

    struct MockMounter {
        mounted: RefCell<Vec<PathBuf>>,
        unmounted: RefCell<Vec<PathBuf>>,
        fail_mount: Option<&'static str>,
        panic_mount: Option<&'static str>,
        fail_unmount: bool,
        fail_output: Option<PathBuf>,
    }

    impl MockMounter {
        fn new() -> Self {
            Self {
                mounted: RefCell::new(Vec::new()),
                unmounted: RefCell::new(Vec::new()),
                fail_mount: None,
                panic_mount: None,
                fail_unmount: false,
                fail_output: None,
            }
        }
    }

    impl ImageMounter for MockMounter {
        fn mount(&self, image: &Path, target: &Path) -> Result<(), Error> {
            let name = image.file_stem().unwrap().to_str().unwrap();
            self.mounted.borrow_mut().push(target.into());
            assert_ne!(self.panic_mount, Some(name), "injected mount panic");
            if self.fail_mount == Some(name) {
                anyhow::bail!("injected mount error");
            }
            let release = target.join("etc/extension-release.d");
            fs::create_dir_all(&release)?;
            fs::write(
                release.join(format!("extension-release.{name}")),
                fs::read(image)?,
            )?;
            if let Some(output) = &self.fail_output {
                fs::create_dir_all(output)?;
            }
            Ok(())
        }

        fn unmount(&self, target: &Path) -> Result<(), Error> {
            self.unmounted.borrow_mut().push(target.into());
            if self.fail_unmount {
                anyhow::bail!("injected unmount error");
            }
            let etc = target.join("etc");
            if etc.exists() {
                fs::remove_dir_all(etc)?;
            }
            Ok(())
        }
    }

    fn setup() -> (TempDir, Paths) {
        let temporary = tempdir().unwrap();
        let paths = Paths::new(
            temporary.path().join("sysroot"),
            temporary.path().join("run"),
            "/var/lib/aclslot".into(),
        )
        .unwrap();
        for slot in ["a", "b"] {
            for kind in ["sysexts", "confexts"] {
                fs::create_dir_all(paths.store().join(slot).join(kind)).unwrap();
            }
        }
        fs::create_dir_all(paths.sysroot().join("usr/lib")).unwrap();
        fs::write(
            paths.sysroot().join("usr/lib/os-release"),
            "ID=acl\nVERSION_ID=3.0\n",
        )
        .unwrap();
        (temporary, paths)
    }

    fn manifest(paths: &Paths, slot: Slot, json: &str) {
        fs::write(
            paths.store().join(slot.name()).join("slot-loader.json"),
            json,
        )
        .unwrap();
    }

    fn image(paths: &Paths, slot: Slot, kind: &str, name: &str) -> PathBuf {
        let path = paths
            .store()
            .join(slot.name())
            .join(kind)
            .join(format!("{name}.raw"));
        fs::write(&path, "ID=acl\nVERSION_ID=3.0\nCONFEXT_ID=defaults\n").unwrap();
        path
    }

    #[test]
    fn native_backend_selects_only_listed_images_and_never_calls_the_mounter() {
        struct NoMounts;
        impl ImageMounter for NoMounts {
            fn mount(&self, _: &Path, _: &Path) -> Result<(), Error> {
                panic!("native backend attempted a mount")
            }
            fn unmount(&self, _: &Path) -> Result<(), Error> {
                panic!("native backend attempted an unmount")
            }
        }
        for slot in [Slot::A, Slot::B] {
            let (_temporary, paths) = setup();
            image(&paths, slot, "sysexts", "runtime");
            image(&paths, slot, "sysexts", "unlisted");
            image(&paths, slot, "confexts", "defaults");
            fs::create_dir_all(paths.runtime().join("extensions")).unwrap();
            fs::write(paths.runtime().join("extensions/operator.raw"), "unrelated").unwrap();
            manifest(
                &paths,
                slot,
                r#"{"version":1,"sysexts":[{"name":"runtime"}],"confexts":[{"name":"defaults"}]}"#,
            );
            fs::create_dir_all(paths.lowerdirs().parent().unwrap()).unwrap();
            fs::write(paths.lowerdirs(), "stale").unwrap();
            run(&paths, slot, ConfextMode::Systemd, &NoMounts).unwrap();
            run(&paths, slot, ConfextMode::Systemd, &NoMounts).unwrap();
            for (native, kind, name) in [
                ("extensions", "sysexts", "runtime"),
                ("confexts", "confexts", "defaults"),
            ] {
                assert_eq!(
                    fs::read_link(paths.runtime().join(native).join(format!("{name}.raw")))
                        .unwrap(),
                    paths
                        .host_store()
                        .join(slot.name())
                        .join(kind)
                        .join(format!("{name}.raw"))
                );
            }
            assert!(fs::symlink_metadata(paths.runtime().join("extensions/unlisted.raw")).is_err());
            assert_eq!(
                fs::read_to_string(paths.runtime().join("extensions/operator.raw")).unwrap(),
                "unrelated"
            );
            assert!(!paths.lowerdirs().exists());
        }
    }

    #[test]
    fn bad_inputs_and_output_conflicts_fail_before_mounting_or_publishing() {
        for failure in ["missing", "symlink", "collision", "output"] {
            let (_temporary, paths) = setup();
            let mounter = MockMounter::new();
            manifest(
                &paths,
                Slot::A,
                r#"{"version":1,"sysexts":[{"name":"runtime"}],"confexts":[{"name":"defaults"}]}"#,
            );
            image(&paths, Slot::A, "sysexts", "runtime");
            match failure {
                "missing" => {}
                "symlink" => symlink(
                    "../sysexts/runtime.raw",
                    paths.store().join("a/confexts/defaults.raw"),
                )
                .unwrap(),
                _ => {
                    image(&paths, Slot::A, "confexts", "defaults");
                    if failure == "collision" {
                        fs::create_dir_all(paths.runtime().join("extensions")).unwrap();
                        fs::write(paths.runtime().join("extensions/runtime.raw"), "unrelated")
                            .unwrap();
                    } else {
                        fs::create_dir_all(paths.lowerdirs()).unwrap();
                    }
                }
            }
            run(&paths, Slot::A, ConfextMode::Overlay, &mounter).unwrap_err();
            assert!(mounter.mounted.borrow().is_empty());
        }
    }

    #[test]
    fn overlay_publishes_manifest_order_after_validation() {
        let (_temporary, paths) = setup();
        let mounter = MockMounter::new();
        image(&paths, Slot::A, "confexts", "z-high");
        image(&paths, Slot::A, "confexts", "a-low");
        manifest(
            &paths,
            Slot::A,
            r#"{"version":1,"confexts":[{"name":"z-high"},{"name":"a-low"}]}"#,
        );
        run(&paths, Slot::A, ConfextMode::Overlay, &mounter).unwrap();
        let prefix = paths.runtime().join("trident-slot-loader/etc/a");
        assert_eq!(
            fs::read_to_string(paths.lowerdirs()).unwrap(),
            format!(
                "{}:{}",
                prefix.join("z-high/etc").display(),
                prefix.join("a-low/etc").display()
            )
        );
        assert!(!paths.runtime().join("confexts").exists());
        assert_eq!(mounter.mounted.borrow().len(), 2);
    }

    #[test]
    fn failed_mount_validation_or_output_rolls_back_but_preserves_the_pending_marker() {
        for failure in ["mount", "compatibility", "publication"] {
            let (_temporary, paths) = setup();
            let mut mounter = MockMounter::new();
            image(&paths, Slot::A, "sysexts", "runtime");
            image(&paths, Slot::A, "confexts", "first");
            let last = image(&paths, Slot::A, "confexts", "last");
            manifest(
                &paths,
                Slot::A,
                r#"{"version":1,"sysexts":[{"name":"runtime"}],"confexts":[{"name":"first"},{"name":"last"}]}"#,
            );
            let pending = paths.store().join("a/slot-loader.pending");
            fs::write(&pending, "").unwrap();
            match failure {
                "mount" => mounter.fail_mount = Some("last"),
                "compatibility" => fs::write(last, "ID=wrong\nCONFEXT_ID=defaults").unwrap(),
                _ => mounter.fail_output = Some(paths.lowerdirs()),
            }
            run(&paths, Slot::A, ConfextMode::Overlay, &mounter).unwrap_err();
            assert!(fs::symlink_metadata(paths.runtime().join("extensions/runtime.raw")).is_err());
            assert!(!paths.lowerdirs().is_file());
            assert!(pending.is_file());
            assert_eq!(mounter.unmounted.borrow().len(), 2);
            assert_eq!(
                mounter
                    .unmounted
                    .borrow()
                    .first()
                    .unwrap()
                    .file_name()
                    .unwrap(),
                "last"
            );
            for mount in mounter.mounted.borrow().iter() {
                assert!(!mount.exists());
            }
        }
    }

    #[test]
    fn incomplete_cleanup_is_reported_without_hiding_the_activation_error() {
        let (_temporary, paths) = setup();
        let mut mounter = MockMounter::new();
        image(&paths, Slot::A, "confexts", "first");
        image(&paths, Slot::A, "confexts", "last");
        manifest(
            &paths,
            Slot::A,
            r#"{"version":1,"confexts":[{"name":"first"},{"name":"last"}]}"#,
        );
        mounter.fail_mount = Some("last");
        mounter.fail_unmount = true;
        let error = run(&paths, Slot::A, ConfextMode::Overlay, &mounter).unwrap_err();
        let chain = format!("{error:#}");
        assert!(
            chain.contains("injected mount error") && chain.contains("injected unmount error"),
            "{chain}"
        );
        assert_eq!(mounter.unmounted.borrow().len(), 2);
        assert!(!paths.lowerdirs().exists());
    }

    #[test]
    fn unwinding_also_cleans_attempted_mounts_without_publishing_a_handoff() {
        let (_temporary, paths) = setup();
        let mut mounter = MockMounter::new();
        image(&paths, Slot::A, "confexts", "first");
        image(&paths, Slot::A, "confexts", "last");
        manifest(
            &paths,
            Slot::A,
            r#"{"version":1,"confexts":[{"name":"first"},{"name":"last"}]}"#,
        );
        mounter.panic_mount = Some("last");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&paths, Slot::A, ConfextMode::Overlay, &mounter).unwrap();
        }));
        assert!(result.is_err());
        assert_eq!(mounter.unmounted.borrow().len(), 2);
        assert!(!paths.lowerdirs().exists());
        for mount in mounter.mounted.borrow().iter() {
            assert!(!mount.exists());
        }
    }

    #[test]
    fn empty_manifest_is_a_runtime_noop_but_clears_its_pending_marker() {
        let (_temporary, paths) = setup();
        let mounter = MockMounter::new();
        manifest(&paths, Slot::A, r#"{"version":1}"#);
        let pending = paths.store().join("a/slot-loader.pending");
        fs::write(&pending, "").unwrap();
        run(&paths, Slot::A, ConfextMode::Overlay, &mounter).unwrap();
        assert!(!paths.runtime().exists());
        assert!(!pending.exists());
    }
}
