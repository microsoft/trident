# ESP Path Constants — Usages

This document lists all non-import usages of the ESP-related path constants
and whether `EngineContext` is accessible at each site.

## No EngineContext Access

Summary of the complicated locations.

| Constant                        | Crate       | Location                                                                              | Function                    | Notes                                                                                    |
| ------------------------------- | ----------- | ------------------------------------------------------------------------------------- | --------------------------- | ---------------------------------------------------------------------------------------- |
| `ESP_MOUNT_POINT_PATH`          | trident_api | [filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353) | `is_esp`                    | Pure method on `FileSystem`; compares mount point to constant                            |
| `ESP_MOUNT_POINT_PATH`          | trident_api | [sample_hc.rs:63](../crates/trident_api/src/samples/sample_hc.rs#L63)                 | `sample_host_configuration` | Sample data builder (×8 occurrences at L63, L116, L313, L524, L985, L1192, L1342, L1524) |
| `ESP_RELATIVE_MOUNT_POINT_PATH` | trident     | [offline_init/mod.rs:489](../crates/trident/src/offline_init/mod.rs#L489)             | `execute`                   | Top-level offline-init command handler; no `EngineContext` in call chain                 |
| `ESP_RELATIVE_MOUNT_POINT_PATH` | trident     | [install_index.rs:15](../crates/trident/src/engine/install_index.rs#L15)              | `next_install_index`        | Pure utility; finds install index from path                                              |

See also [Annex: `is_esp()` Usages](#annex-is_esp-usages) for downstream callers
of the `is_esp` method.

---

## Product Code Usages

### `ESP_MOUNT_POINT_PATH`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L94) as `/boot/efi`.

#### Usages

| Crate       | Location                                                                              | Function                    | EngineContext Access | Notes                                                                                    |
| ----------- | ------------------------------------------------------------------------------------- | --------------------------- | -------------------- | ---------------------------------------------------------------------------------------- |
| trident     | [ab_update.rs:159](../crates/trident/src/engine/ab_update.rs#L159)                    | `finalize_update`           | Yes                  | `ctx` built locally at L142                                                              |
| trident     | [clean_install.rs:309](../crates/trident/src/engine/clean_install.rs#L309)            | `finalize_clean_install`    | Yes                  | `ctx` built locally at L283                                                              |
| trident     | [uki.rs:62](../crates/trident/src/engine/boot/uki.rs#L62)                             | `stage_uki_on_esp`          | Indirect             | Caller `copy_file_artifacts` in esp.rs has `ctx`                                         |
| trident     | [uki.rs:148](../crates/trident/src/engine/boot/uki.rs#L148)                           | `prepare_esp_for_uki`       | Indirect             | Caller `copy_file_artifacts` in esp.rs has `ctx`                                         |
| trident_api | [filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353) | `is_esp`                    | No                   | Pure method on `FileSystem`; compares mount point to constant                            |
| trident_api | [sample_hc.rs:63](../crates/trident_api/src/samples/sample_hc.rs#L63)                 | `sample_host_configuration` | No                   | Sample data builder (×8 occurrences at L63, L116, L313, L524, L985, L1192, L1342, L1524) |

---

### `ESP_RELATIVE_MOUNT_POINT_PATH`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L91) as `boot/efi`.

#### Usages

| Crate       | Location                                                                               | Function                            | EngineContext Access | Notes                                                                    |
| ----------- | -------------------------------------------------------------------------------------- | ----------------------------------- | -------------------- | ------------------------------------------------------------------------ |
| trident_api | [constants.rs:95](../crates/trident_api/src/constants.rs#L95)                          | *(const)*                           | N/A                  | Used to define `ESP_MOUNT_POINT_PATH`                                    |
| trident     | [offline_init/mod.rs:489](../crates/trident/src/offline_init/mod.rs#L489)              | `execute`                           | No                   | Top-level offline-init command handler; no `EngineContext` in call chain |
| trident     | [esp.rs:415](../crates/trident/src/subsystems/esp.rs#L415)                             | `copy_boot_files_for_uefi_fallback` | Indirect             | Caller `set_uefi_fallback_contents` has `ctx`                            |
| trident     | [esp.rs:693](../crates/trident/src/subsystems/esp.rs#L693)                             | `generate_efi_bin_base_dir_path`    | Yes                  | First param is `ctx: &EngineContext`                                     |
| trident     | [install_index.rs:15](../crates/trident/src/engine/install_index.rs#L15)               | `next_install_index`                | No                   | Pure utility; finds install index from path                              |
| trident     | [manual_rollback/mod.rs:293](../crates/trident/src/engine/manual_rollback/mod.rs#L293) | `finalize_ab`                       | Yes                  | Param `engine_context: &EngineContext`                                   |

---

### `ROOT_EFI_DIRECTORY`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L70) as `efi`.

#### Summary

#### Usages

| Crate       | Location                                                      | Function  | EngineContext Access | Notes                                          |
| ----------- | ------------------------------------------------------------- | --------- | -------------------- | ---------------------------------------------- |
| trident_api | [constants.rs:91](../crates/trident_api/src/constants.rs#L91) | *(const)* | N/A                  | Used to define `ESP_RELATIVE_MOUNT_POINT_PATH` |

---

## Test Usages

### `ESP_MOUNT_POINT_PATH`

| File                                                     | Instances |
| -------------------------------------------------------- | --------- |
| crates/osutils/src/tabfile.rs                            | 2         |
| crates/trident_api/src/config/host/storage/filesystem.rs | 1         |

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
and uses `ESP_MOUNT_POINT_PATH` internally.

### Product Code

| Crate       | Location                                                                                                            | Function                            | Description                                                        |
| ----------- | ------------------------------------------------------------------------------------------------------------------- | ----------------------------------- | ------------------------------------------------------------------ |
| trident     | [storage/filesystem.rs:60](../crates/trident/src/engine/storage/filesystem.rs#L60)                                  | `block_devices_needing_fs_creation` | Guard in pattern match; decides if ESP needs filesystem recreation |
| trident     | [storage/image.rs:220](../crates/trident/src/engine/storage/image.rs#L220)                                          | `filesystems_from_image`            | Skips ESP deployment when not using raw COSI storage               |
| trident     | [cosi/metadata.rs:136](../crates/trident/src/osimage/cosi/metadata.rs#L136)                                         | `get_esp_filesystem`                | Filters images list to find the ESP filesystem                     |
| trident     | [cosi/metadata.rs:158](../crates/trident/src/osimage/cosi/metadata.rs#L158)                                         | `get_regular_filesystems`           | Filters out ESP to iterate only non-ESP filesystems                |
| trident     | [storage/osimage.rs:152](../crates/trident/src/subsystems/storage/osimage.rs#L152)                                  | `validate_filesystems`              | Includes ESP in required filesystems map for validation            |
| trident_api | [storage_graph/conversions.rs:121](../crates/trident_api/src/config/host/storage/storage_graph/conversions.rs#L121) | `from` (`BlkDevReferrerKind`)       | Classifies filesystem as `FileSystemEsp` in the storage graph      |

### Test Code

| File                                                     | Instances | Test Function                      |
| -------------------------------------------------------- | --------- | ---------------------------------- |
| crates/trident_api/src/config/host/storage/filesystem.rs | 4         | `test_filesystem_mount_point_path` |
