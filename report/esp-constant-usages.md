# ESP Path Constants — Usages

This document lists all non-import usages of the ESP-related path constants
and whether `EngineContext` is accessible at each site.

## Legend

| Symbol | EngineContext Access                                        |
| ------ | ----------------------------------------------------------- |
| ✅      | **Yes** — directly available as parameter or local variable |
| 🔗      | **Indirect** — not in function, but immediate caller has it |
| ❌      | **No** — no `EngineContext` accessible nearby               |
| ➖      | **N/A** — const definition or similar; not applicable       |

## No EngineContext Access

Summary of the complicated locations.

| Item# | Constant                        | Crate       | Location                                                                                          | Function                      | Notes                                                                                    |
| ----- | ------------------------------- | ----------- | ------------------------------------------------------------------------------------------------- | ----------------------------- | ---------------------------------------------------------------------------------------- |
| 1     | `ESP_MOUNT_POINT_PATH`          | trident_api | [rules/mod.rs:524](../crates/trident_api/src/config/host/storage/storage_graph/rules/mod.rs#L524) | `expected_partition_type`     | Returns allowed partition type for ESP mount point                                       |
| 2     | `ESP_MOUNT_POINT_PATH`          | trident_api | [storage/mod.rs:188](../crates/trident_api/src/config/host/storage/mod.rs#L188)                   | `Storage::validate`           | Validates ESP volume presence in storage config                                          |
| 3     | `ESP_MOUNT_POINT_PATH`          | trident_api | [storage/mod.rs:507](../crates/trident_api/src/config/host/storage/mod.rs#L507)                   | `Storage::esp_filesystem`     | Returns reference to ESP device_id and filesystem                                        |
| 4     | `ESP_MOUNT_POINT_PATH`          | trident_api | [filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353)             | `FileSystem::is_esp`          | Pure method on `FileSystem`; compares mount point to constant                            |
| 5     | `ESP_MOUNT_POINT_PATH`          | trident_api | [sample_hc.rs:63](../crates/trident_api/src/samples/sample_hc.rs#L63)                             | `sample_host_configuration`   | Sample data builder (×8 occurrences at L63, L116, L313, L524, L985, L1192, L1342, L1524) |
| 6     | `ESP_MOUNT_POINT_PATH`          | trident     | [context/filesystem.rs:178](../crates/trident/src/engine/context/filesystem.rs#L178)              | `FileSystemData::is_esp`      | Pure method; checks if filesystem mount equals ESP path                                  |
| 7     | `ESP_MOUNT_POINT_PATH`          | trident     | [context/filesystem.rs:258](../crates/trident/src/engine/context/filesystem.rs#L258)              | `FileSystemDataImage::is_esp` | Pure method; checks ESP mount path equality                                              |
| 20    | `ESP_RELATIVE_MOUNT_POINT_PATH` | trident     | [offline_init/mod.rs:489](../crates/trident/src/offline_init/mod.rs#L489)                         | `execute`                     | Top-level offline-init command handler; no `EngineContext` in call chain                 |
| 23    | `ESP_RELATIVE_MOUNT_POINT_PATH` | trident     | [install_index.rs:15](../crates/trident/src/engine/install_index.rs#L15)                          | `next_install_index`          | Pure utility; finds install index from path                                              |

See also 
- [Annex: `is_esp()` Usages](#annex-is_esp-usages) for downstream callers of the
  `is_esp` method.

---

## Product Code Usages

### `ESP_MOUNT_POINT_PATH`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L94) as `/boot/efi`.

#### Usages

| Item# | Crate       | Location                                                                                          | Function                                         | Ctx | Notes                                                                        |
| ----- | ----------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------ | --- | ---------------------------------------------------------------------------- |
| 1     | trident_api | [rules/mod.rs:524](../crates/trident_api/src/config/host/storage/storage_graph/rules/mod.rs#L524) | `expected_partition_type`                        | ❌   | Returns allowed partition type for ESP mount point                           |
| 2     | trident_api | [storage/mod.rs:188](../crates/trident_api/src/config/host/storage/mod.rs#L188)                   | `Storage::validate`                              | ❌   | Validates ESP volume presence in storage config                              |
| 3     | trident_api | [storage/mod.rs:507](../crates/trident_api/src/config/host/storage/mod.rs#L507)                   | `Storage::esp_filesystem`                        | ❌   | Returns reference to ESP device_id and filesystem                            |
| 4     | trident_api | [filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353)             | `FileSystem::is_esp`                             | ❌   | Pure method; compares mount point to constant                                |
| 5     | trident_api | [sample_hc.rs:63](../crates/trident_api/src/samples/sample_hc.rs#L63)                             | `sample_host_configuration`                      | ❌   | Sample data builder (×8 at L63, L116, L313, L524, L985, L1192, L1342, L1524) |
| 6     | trident     | [context/filesystem.rs:178](../crates/trident/src/engine/context/filesystem.rs#L178)              | `FileSystemData::is_esp`                         | ❌   | Pure method; checks if filesystem mount equals ESP path                      |
| 7     | trident     | [context/filesystem.rs:258](../crates/trident/src/engine/context/filesystem.rs#L258)              | `FileSystemDataImage::is_esp`                    | ❌   | Pure method; checks ESP mount path equality                                  |
| 8     | trident     | [context/filesystem.rs:357](../crates/trident/src/engine/context/filesystem.rs#L357)              | `EngineContext::esp_filesystem`                  | ✅   | Finds ESP filesystem in image filesystems                                    |
| 9     | trident     | [grub.rs:84](../crates/trident/src/engine/boot/grub.rs#L84)                                       | `update_configs`                                 | ✅   | Constructs GRUB boot entry config path on ESP                                |
| 10    | trident     | [bootentries.rs:289](../crates/trident/src/engine/bootentries.rs#L289)                            | `create_boot_entries_for_rebuilt_esp_partitions` | ✅   | Boot entry creation on ESP for RAID recovery                                 |
| 11    | trident     | [encryption.rs:306](../crates/trident/src/engine/storage/encryption.rs#L306)                      | `get_binary_paths_pcrlock`                       | ✅   | Gets ESP path for UKI/bootloader binary discovery                            |
| 12    | trident     | [encryption.rs:455](../crates/trident/src/engine/storage/encryption.rs#L455)                      | `get_bootloader_paths`                           | ✅   | Constructs bootloader paths in target OS during A/B update                   |
| 13    | trident     | [verity.rs:185](../crates/trident/src/engine/storage/verity.rs#L185)                              | `open_verity_device_with_signature`              | ✅   | Validates signature file is NOT on ESP mount point (×2 at L185, L187)        |
| 14    | trident     | [image.rs:75](../crates/trident/src/engine/storage/image.rs#L75)                                  | `deploy_images`                                  | ✅   | Maps ESP filesystem in raw COSI storage mode                                 |
| 15    | trident     | [uki.rs:62](../crates/trident/src/engine/boot/uki.rs#L62)                                         | `stage_uki_on_esp`                               | 🔗   | UKI staging path construction; caller has `ctx`                              |
| 16    | trident     | [uki.rs:148](../crates/trident/src/engine/boot/uki.rs#L148)                                       | `prepare_esp_for_uki`                            | 🔗   | ESP preparation for UKI; caller has `ctx`                                    |
| 17    | trident     | [ab_update.rs:159](../crates/trident/src/engine/ab_update.rs#L159)                                | `finalize_update`                                | ✅   | `ctx` built locally at L142                                                  |
| 18    | trident     | [clean_install.rs:309](../crates/trident/src/engine/clean_install.rs#L309)                        | `finalize_clean_install`                         | ✅   | `ctx` built locally at L283                                                  |

---

### `ESP_RELATIVE_MOUNT_POINT_PATH`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L91) as `boot/efi`.

#### Usages

| Item# | Crate       | Location                                                                               | Function                            | Ctx | Notes                                                                    |
| ----- | ----------- | -------------------------------------------------------------------------------------- | ----------------------------------- | --- | ------------------------------------------------------------------------ |
| 19    | trident_api | [constants.rs:95](../crates/trident_api/src/constants.rs#L95)                          | *(const)*                           | ➖   | Used to define `ESP_MOUNT_POINT_PATH`                                    |
| 20    | trident     | [offline_init/mod.rs:489](../crates/trident/src/offline_init/mod.rs#L489)              | `execute`                           | ❌   | Top-level offline-init command handler; no `EngineContext` in call chain |
| 21    | trident     | [esp.rs:415](../crates/trident/src/subsystems/esp.rs#L415)                             | `copy_boot_files_for_uefi_fallback` | 🔗   | Caller `set_uefi_fallback_contents` has `ctx`                            |
| 22    | trident     | [esp.rs:693](../crates/trident/src/subsystems/esp.rs#L693)                             | `generate_efi_bin_base_dir_path`    | ✅   | First param is `ctx: &EngineContext`                                     |
| 23    | trident     | [install_index.rs:15](../crates/trident/src/engine/install_index.rs#L15)               | `next_install_index`                | ❌   | Pure utility; finds install index from path                              |
| 24    | trident     | [manual_rollback/mod.rs:293](../crates/trident/src/engine/manual_rollback/mod.rs#L293) | `finalize_ab`                       | ✅   | Param `engine_context: &EngineContext`                                   |

---

### `ROOT_EFI_DIRECTORY`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L70) as `efi`.

#### Usages

| Item# | Crate       | Location                                                      | Function  | Ctx | Notes                                          |
| ----- | ----------- | ------------------------------------------------------------- | --------- | --- | ---------------------------------------------- |
| 25    | trident_api | [constants.rs:91](../crates/trident_api/src/constants.rs#L91) | *(const)* | ➖   | Used to define `ESP_RELATIVE_MOUNT_POINT_PATH` |

---

## Test Usages

### `ESP_MOUNT_POINT_PATH`

| File                                                                         | Instances |
| ---------------------------------------------------------------------------- | --------- |
| crates/osutils/src/tabfile.rs                                                | 2         |
| crates/trident_api/src/config/host/storage/filesystem.rs                     | 1         |
| crates/trident_api/src/config/host/storage/mod.rs                            | 5         |
| crates/trident_api/src/config/host/storage/storage_graph/validation_tests.rs | 2         |
| crates/trident/src/engine/boot/grub.rs                                       | 1         |
| crates/trident/src/engine/boot/uki.rs                                        | 5         |
| crates/trident/src/engine/bootentries.rs                                     | 3         |
| crates/trident/src/engine/storage/encryption.rs                              | 3         |
| crates/trident/src/subsystems/storage/fstab.rs                               | 5         |
| crates/trident/src/subsystems/storage/osimage.rs                             | 2         |

### `ESP_RELATIVE_MOUNT_POINT_PATH`

| File                                       | Instances |
| ------------------------------------------ | --------- |
| crates/trident/src/subsystems/esp.rs       | 3         |
| crates/trident/src/engine/install_index.rs | 1         |
| crates/trident/src/engine/newroot.rs       | 2         |

### `ROOT_EFI_DIRECTORY`

No test usages.

---

## Annex: `is_esp()` Usages

The `is_esp()` method is defined on `FileSystem` in
[filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353)
and uses `ESP_MOUNT_POINT_PATH` internally. Note that there are also
`is_esp()` methods on `FileSystemData` and `FileSystemDataImage` in
[context/filesystem.rs](../crates/trident/src/engine/context/filesystem.rs)
that use the same constant directly.

### Product Code

| Item# | Crate       | Location                                                                                                            | Function                            | Description                                                        |
| ----- | ----------- | ------------------------------------------------------------------------------------------------------------------- | ----------------------------------- | ------------------------------------------------------------------ |
| 26    | trident     | [storage/filesystem.rs:60](../crates/trident/src/engine/storage/filesystem.rs#L60)                                  | `block_devices_needing_fs_creation` | Guard in pattern match; decides if ESP needs filesystem recreation |
| 27    | trident     | [storage/image.rs:220](../crates/trident/src/engine/storage/image.rs#L220)                                          | `filesystems_from_image`            | Skips ESP deployment when not using raw COSI storage               |
| 28    | trident     | [cosi/metadata.rs:136](../crates/trident/src/osimage/cosi/metadata.rs#L136)                                         | `get_esp_filesystem`                | Filters images list to find the ESP filesystem                     |
| 29    | trident     | [cosi/metadata.rs:158](../crates/trident/src/osimage/cosi/metadata.rs#L158)                                         | `get_regular_filesystems`           | Filters out ESP to iterate only non-ESP filesystems                |
| 30    | trident     | [storage/osimage.rs:152](../crates/trident/src/subsystems/storage/osimage.rs#L152)                                  | `validate_filesystems`              | Includes ESP in required filesystems map for validation            |
| 31    | trident_api | [storage_graph/conversions.rs:121](../crates/trident_api/src/config/host/storage/storage_graph/conversions.rs#L121) | `from` (`BlkDevReferrerKind`)       | Classifies filesystem as `FileSystemEsp` in the storage graph      |

### Test Code

| File                                                     | Instances | Test Function                      |
| -------------------------------------------------------- | --------- | ---------------------------------- |
| crates/trident_api/src/config/host/storage/filesystem.rs | 4         | `test_filesystem_mount_point_path` |
