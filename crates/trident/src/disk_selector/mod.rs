use std::{collections::HashSet, fs};

use anyhow::Context;

use trident_api::{
    config::HostConfiguration,
    error::{InvalidInputError, ReportError, TridentError, TridentResultExt},
    storage_graph::graph::StorageGraph,
};

use crate::osimage::OsImage;

mod capacity;
mod inventory;
mod select;

/// Rejects selectors that staging should already have resolved.
pub(crate) fn check_no_selectors(spec: &HostConfiguration) -> Result<(), TridentError> {
    for disk in &spec.storage.disks {
        if disk.selector.is_some() {
            return Err(TridentError::new(InvalidInputError::DiskSelection {
                disk_id: disk.id.clone(),
            }))
            .message("Saved disk selectors must be resolved before finalization");
        }
    }
    Ok(())
}

/// Resolves install selectors to current device paths in the input configuration.
///
/// Uses COSI metadata to size targets, reserves explicit disks, then applies
/// selectors in configuration order and removes their selector fields. Does
/// nothing when no selectors are present; does not read or write persisted state.
pub(crate) fn resolve_install(
    spec: &mut HostConfiguration,
    image: &OsImage,
    graph: &StorageGraph,
) -> Result<(), TridentError> {
    if !spec
        .storage
        .disks
        .iter()
        .any(|disk| disk.selector.is_some())
    {
        // Exit early if no disks have selectors
        return Ok(());
    }

    plan(spec, image, graph)
}

/// Computes HC disk sizes and image-based grow minima, reserves explicit targets,
/// and discovers candidates before resolving and validating the resulting HC.
fn plan(
    spec: &mut HostConfiguration,
    image: &OsImage,
    graph: &StorageGraph,
) -> Result<(), TridentError> {
    // Mapping disk IDs to their required capacities based on the image and storage spec.
    let requirements = capacity::requirements(
        &spec.storage,
        image,
        graph,
        spec.storage
            .disks
            .iter()
            .filter(|disk| disk.selector.is_some()),
    )
    .structured(InvalidInputError::DiskSelectionPreparation)?;

    // Get a set of explicitly reserved (i.e., unavailable for selection) disks
    // based on the current storage spec.
    let reserved = spec
        .storage
        .disks
        .iter()
        .filter(|disk| disk.selector.is_none())
        .map(|disk| {
            fs::canonicalize(&disk.device).with_context(|| {
                format!("Cannot reserve explicit disk '{}'", disk.device.display())
            })
        })
        .collect::<Result<HashSet<_>, _>>()
        .structured(InvalidInputError::DiskSelectionPreparation)?;

    // Discover all candidate disks available on the system.
    let candidates =
        inventory::discover().structured(InvalidInputError::DiskSelectionPreparation)?;

    // Select disks for the current configuration based on requirements, candidates, and reserved disks.
    select::select(spec, &requirements, &candidates, reserved)?;

    // Validate the resulting host configuration after disk selection.
    spec.validate()
        .map_err(Into::into)
        .message("Invalid Host Configuration after disk selection")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::{
        config::{Disk, DiskSelector, Partition, Storage},
        error::ErrorKind,
    };

    use crate::osimage::mock::MockOsImage;

    #[test]
    fn preparation_failures_do_not_identify_an_arbitrary_selector() {
        let mut input = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".into(),
                    selector: Some(DiskSelector::default()),
                    partitions: vec![Partition::new("huge", u64::MAX - 4095)],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let graph = input.storage.build_graph().unwrap();
        let error =
            resolve_install(&mut input, &OsImage::mock(MockOsImage::new()), &graph).unwrap_err();
        assert_eq!(
            error.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::DiskSelectionPreparation)
        );
        assert!(format!("{error:?}").contains("Disk layout size overflow"));
    }
}
