//! Native activation only: publish links for systemd without dissecting or mounting images.

use std::{fs, io, os::unix::fs::symlink};

use anyhow::{ensure, Context, Error};
use log::{debug, info};

use crate::{filesystem, selection::NativeLink};

/// Tracks only links created by this invocation; matching pre-existing links are never rolled back.
pub(super) struct LinkSet {
    links: Vec<NativeLink>,
    created: Vec<usize>,
}

impl LinkSet {
    pub(super) fn preflight(links: Vec<NativeLink>) -> Result<Self, Error> {
        for link in &links {
            inspect(link)?;
        }
        Ok(Self {
            links,
            created: Vec::new(),
        })
    }

    pub(super) fn publish(&mut self) -> Result<(), Error> {
        for (index, link) in self.links.iter().enumerate() {
            if inspect(link)? {
                debug!(
                    "event=link_reused destination={} target={}",
                    link.destination.display(),
                    link.target.display()
                );
                continue;
            }
            filesystem::create_directory(link.destination.parent().context("link has no parent")?)?;
            symlink(&link.target, &link.destination).with_context(|| {
                format!(
                    "linking {} -> {}",
                    link.destination.display(),
                    link.target.display()
                )
            })?;
            self.created.push(index);
            debug!(
                "event=link_created destination={} target={}",
                link.destination.display(),
                link.target.display()
            );
        }
        info!(
            "event=links_ready selected={} created={}",
            self.links.len(),
            self.created.len()
        );
        Ok(())
    }

    pub(super) fn rollback(&mut self) -> Vec<Error> {
        let mut errors = Vec::new();
        while let Some(index) = self.created.pop() {
            let link = &self.links[index];
            let result: Result<(), Error> = (|| {
                if inspect(link)? {
                    fs::remove_file(&link.destination).with_context(|| {
                        format!("removing created link {}", link.destination.display())
                    })?;
                    debug!(
                        "event=link_removed destination={}",
                        link.destination.display()
                    );
                }
                Ok(())
            })();
            if let Err(error) = result {
                errors.push(error);
            }
        }
        errors
    }
}

/// Returns true only for the exact expected symlink, including dangling post-switch-root targets.
fn inspect(link: &NativeLink) -> Result<bool, Error> {
    filesystem::directory_exists(link.destination.parent().context("link has no parent")?)?;
    match fs::symlink_metadata(&link.destination) {
        Ok(metadata) => {
            ensure!(
                metadata.is_symlink() && fs::read_link(&link.destination)? == link.target,
                "conflicting activation path {}",
                link.destination.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("inspecting {}", link.destination.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(root: &std::path::Path, name: &str) -> NativeLink {
        NativeLink {
            destination: root.join(format!("{name}.raw")),
            target: format!("/var/lib/aclslot/a/sysexts/{name}.raw").into(),
        }
    }

    #[test]
    fn rollback_removes_only_new_links_and_preserves_a_replaced_destination() {
        let root = tempfile::tempdir().unwrap();
        let existing = link(root.path(), "existing");
        symlink(&existing.target, &existing.destination).unwrap();
        let mut links = LinkSet::preflight(vec![
            existing,
            link(root.path(), "new"),
            link(root.path(), "replaced"),
        ])
        .unwrap();
        links.publish().unwrap();
        fs::remove_file(root.path().join("replaced.raw")).unwrap();
        fs::write(root.path().join("replaced.raw"), "other writer").unwrap();
        let errors = links.rollback();
        assert_eq!(errors.len(), 1);
        assert!(fs::read_link(root.path().join("existing.raw")).is_ok());
        assert!(fs::symlink_metadata(root.path().join("new.raw")).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("replaced.raw")).unwrap(),
            "other writer"
        );
    }
}
