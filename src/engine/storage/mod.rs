use log::{debug, info, trace, warn};

use osutils::{e2fsck, hashing_reader::compute_file_hash, lsblk};
use sysdefs::filesystems::{KernelFilesystemType, RealFilesystemType};
use trident_api::{
    constants::internal_params::PRE_REBOOT_CHECKS,
    error::{ReportError, ServicingError, TridentError, TridentResultExt},
    status::HostStatus,
};

mod common;
pub mod encryption;
mod filesystem;
pub mod image;
pub mod partitioning;
pub mod raid;
pub mod rebuild;
mod swap;
pub mod verity;

use super::EngineContext;

#[tracing::instrument(skip_all)]
pub(super) fn create_block_devices(ctx: &mut EngineContext) -> Result<(), TridentError> {
    trace!(
        "Mount points: {:?}",
        ctx.mounted_filesystems()
            .map(|fs| fs.description())
            .collect::<Vec<_>>()
    );

    debug!("Initializing block devices");

    // Close verity devices and encrypted volumes before stopping RAID
    // arrays, as both can sit on top of RAID arrays.
    close_pre_existing_devices(ctx).message("Closing pre-existing block devices failed")?;

    partitioning::create_partitions(ctx).structured(ServicingError::CreatePartitions)?;
    raid::create_sw_raid(ctx, &ctx.spec).structured(ServicingError::CreateRaid)?;
    encryption::create_encrypted_devices(ctx, &ctx.spec)
        .message("Failed to create and open encrypted devices")?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn close_pre_existing_devices(ctx: &EngineContext) -> Result<(), TridentError> {
    debug!("Closing pre-existing block devices");

    // Close verity devices and encrypted volumes before stopping RAID
    // arrays, as both can sit on top of RAID arrays.
    verity::stop_trident_servicing_devices(&ctx.spec).structured(ServicingError::CleanupVerity)?;
    encryption::close_pre_existing_encrypted_volumes(&ctx.spec)
        .structured(ServicingError::CleanupEncryption)?;
    raid::stop_pre_existing_raid_arrays(&ctx.spec).structured(ServicingError::CleanupRaid)?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn initialize_block_devices(ctx: &EngineContext) -> Result<(), TridentError> {
    // Deploy images on block devices as specified in the configuration.
    image::deploy_images(ctx)?;

    // Create filesystems on block devices as specified in the configuration.
    filesystem::create_filesystems(ctx).structured(ServicingError::CreateFilesystems)?;

    // Create swap spaces on block devices as specified in the configuration.
    swap::create_swap(ctx).structured(ServicingError::CreateSwap)?;

    // Assumes that images are already in place (data and hash), so that it can
    // assemble the verity devices.
    verity::setup_verity_devices(ctx).structured(ServicingError::CreateVerity)?;

    Ok(())
}

pub(super) fn check_block_devices(host_status: &HostStatus) {
    if !host_status.spec.internal_params.get_flag(PRE_REBOOT_CHECKS) {
        return;
    }

    for (id, path) in &host_status.partition_paths {
        let Ok(canonical) = path.canonicalize() else {
            warn!(
                "Block device '{id}' (path '{}'): No longer exists",
                path.display()
            );
            continue;
        };

        let Ok((length, sha384)) = compute_file_hash(&canonical) else {
            warn!(
                "Block device '{id}' (path '{}' -> '{}'): Failed to compute hash",
                path.display(),
                canonical.display()
            );
            continue;
        };

        let Ok(block_device) = lsblk::get(&canonical) else {
            warn!(
                "Block device '{id}' (path '{}' -> '{}'): Failed to find block device information with lsblk; skipping filesystem check",
                path.display(),
                canonical.display()
            );
            continue;
        };

        let fsck_status = match block_device
            .fstype
            .and_then(|fs_type| KernelFilesystemType::from(fs_type.as_str()).try_as_real())
        {
            Some(RealFilesystemType::Ext4) => {
                if let Err(e) = e2fsck::check(&canonical) {
                    format!(", e2fsck failed: {e:?}")
                } else {
                    ", e2fsck OK".to_string()
                }
            }
            _ => "".to_string(),
        };

        info!(
            "Block device '{id}' (path '{}' -> '{}'): Size = {length} bytes, sha384 = {sha384}{fsck_status}",
            path.display(),
            canonical.display(),
        );
    }
}
