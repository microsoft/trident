# ESP Path Constants — Usages

This document lists all non-import usages of the ESP-related path constants
and whether `EngineContext` is accessible at each site.

## Legend

Status:

| Symbol  | Status                                                                  |
| ------- | ----------------------------------------------------------------------- |
| ✅       | **OK** — usage has been removed                                         |
| ☑️       | This has been reviewed and determined to be a valid use of the constant |
| (blank) | **Untouched** — usage still present                                     |

EngineContext Availability:

| Symbol | EngineContext Access                                        |
| ------ | ----------------------------------------------------------- |
| ✅      | **Yes** — directly available as parameter or local variable |
| 🔗      | **Indirect** — not in function, but immediate caller has it |
| ❌      | **No** — no `EngineContext` accessible nearby               |
| ➖      | **N/A** — const definition or similar; not applicable       |

## No EngineContext Access

Summary of the complicated locations.

| Item# | Constant                        | Crate       | Location                                                                                          | Function                      | Notes                                                                                     | Status |
| ----- | ------------------------------- | ----------- | ------------------------------------------------------------------------------------------------- | ----------------------------- | ----------------------------------------------------------------------------------------- | ------ |
| 1     | `ESP_MOUNT_POINT_PATH`          | trident_api | [rules/mod.rs:524](../crates/trident_api/src/config/host/storage/storage_graph/rules/mod.rs#L524) | `expected_partition_type`     | ~~Returns allowed partition type for ESP mount point~~ Now takes `esp_mount_path` param   | ✅      |
| 2     | `ESP_MOUNT_POINT_PATH`          | trident_api | [storage/mod.rs:188](../crates/trident_api/src/config/host/storage/mod.rs#L188)                   | `Storage::validate`           | Validates ESP volume presence in storage config                                           | ✅      |
| 3     | `ESP_MOUNT_POINT_PATH`          | trident_api | [storage/mod.rs:545](../crates/trident_api/src/config/host/storage/mod.rs#L545)                   | `Storage::esp_filesystem`     | Returns reference to ESP device_id and filesystem                                         | ✅      |
| 4     | `ESP_MOUNT_POINT_PATH`          | trident_api | [filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353)             | `FileSystem::is_esp`          | ~~Pure method~~ Now a field; set by `Storage::initialize()`                               | ✅      |
| 5     | `ESP_MOUNT_POINT_PATH`          | trident_api | [sample_hc.rs:63](../crates/trident_api/src/samples/sample_hc.rs#L63)                             | `sample_host_configuration`   | Sample data builder (×8 occurrences at L63, L118, L317, L533, L1001, L1216, L1368, L1553) | ☑️[1]   |
| 5b    | `ESP_MOUNT_POINT_PATH`          | trident_api | [filesystem.rs:61](../crates/trident_api/src/config/host/storage/filesystem.rs#L61)               | `fs_serde` (deserialization)  | `DEFAULT_ESP_MOUNT_PATH` alias; used to detect default ESP mount during serde             | ☑️[2]   |
| 6     | `ESP_MOUNT_POINT_PATH`          | trident     | [context/filesystem.rs:178](../crates/trident/src/engine/context/filesystem.rs#L178)              | `FileSystemData::is_esp`      | Pure method; checks if filesystem mount equals ESP path                                   | ✅      |
| 7     | `ESP_MOUNT_POINT_PATH`          | trident     | [context/filesystem.rs:258](../crates/trident/src/engine/context/filesystem.rs#L258)              | `FileSystemDataImage::is_esp` | Pure method; checks ESP mount path equality                                               | ✅      |
| 20    | `ESP_RELATIVE_MOUNT_POINT_PATH` | trident     | [init/offline/mod.rs:527](../crates/trident/src/init/offline/mod.rs#L527)                         | `execute`                     | Top-level offline-init command handler; no `EngineContext` in call chain                  | ✅      |
| 23    | `ESP_RELATIVE_MOUNT_POINT_PATH` | trident     | [install_index.rs:15](../crates/trident/src/engine/install_index.rs#L15)                          | `next_install_index`          | Pure utility; finds install index from path                                               | ✅      |

---

## Product Code Usages

### `ESP_MOUNT_POINT_PATH`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L94) as `/boot/efi`.

#### Usages

| Item# | Crate       | Location                                                                                          | Function                                         | Ctx | Notes                                                                                   | Status |
| ----- | ----------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------ | --- | --------------------------------------------------------------------------------------- | ------ |
| 1     | trident_api | [rules/mod.rs:524](../crates/trident_api/src/config/host/storage/storage_graph/rules/mod.rs#L524) | `expected_partition_type`                        | ❌   | ~~Returns allowed partition type for ESP mount point~~ Now takes `esp_mount_path` param | ✅      |
| 2     | trident_api | [storage/mod.rs:188](../crates/trident_api/src/config/host/storage/mod.rs#L188)                   | `Storage::validate`                              | ❌   | Validates ESP volume presence in storage config                                         | ✅      |
| 3     | trident_api | [storage/mod.rs:545](../crates/trident_api/src/config/host/storage/mod.rs#L545)                   | `Storage::esp_filesystem`                        | ❌   | Returns reference to ESP device_id and filesystem                                       | ✅      |
| 4     | trident_api | [filesystem.rs:353](../crates/trident_api/src/config/host/storage/filesystem.rs#L353)             | `FileSystem::is_esp`                             | ❌   | ~~Pure method~~ Now a field; set by `Storage::initialize()`                             | ✅      |
| 5     | trident_api | [sample_hc.rs:63](../crates/trident_api/src/samples/sample_hc.rs#L63)                             | `sample_host_configuration`                      | ❌   | Sample data builder (×8 at L63, L118, L317, L533, L1001, L1216, L1368, L1553)           | ☑️[1]   |
| 5b    | trident_api | [filesystem.rs:61](../crates/trident_api/src/config/host/storage/filesystem.rs#L61)               | `fs_serde` (deserialization)                     | ❌   | `DEFAULT_ESP_MOUNT_PATH` alias; used to detect default ESP mount during serde           | ☑️[2]   |
| 6     | trident     | [context/filesystem.rs:178](../crates/trident/src/engine/context/filesystem.rs#L178)              | `FileSystemData::is_esp`                         | ❌   | Pure method; checks if filesystem mount equals ESP path                                 | ✅      |
| 7     | trident     | [context/filesystem.rs:258](../crates/trident/src/engine/context/filesystem.rs#L258)              | `FileSystemDataImage::is_esp`                    | ❌   | Pure method; checks ESP mount path equality                                             | ✅      |
| 8     | trident     | [context/filesystem.rs:346](../crates/trident/src/engine/context/filesystem.rs#L346)              | `EngineContext::esp_filesystem`                  | ✅   | Finds ESP filesystem in image filesystems                                               | ✅      |
| 9     | trident     | [grub.rs:84](../crates/trident/src/engine/boot/grub.rs#L84)                                       | `update_configs`                                 | ✅   | Constructs GRUB boot entry config path on ESP                                           | ✅      |
| 10    | trident     | [bootentries.rs:289](../crates/trident/src/engine/bootentries.rs#L289)                            | `create_boot_entries_for_rebuilt_esp_partitions` | ✅   | Boot entry creation on ESP for RAID recovery                                            | ✅      |
| 11    | trident     | [encryption.rs:306](../crates/trident/src/engine/storage/encryption.rs#L306)                      | `get_binary_paths_pcrlock`                       | ✅   | Gets ESP path for UKI/bootloader binary discovery                                       | ✅      |
| 12    | trident     | [encryption.rs:455](../crates/trident/src/engine/storage/encryption.rs#L455)                      | `get_bootloader_paths`                           | ✅   | Constructs bootloader paths in target OS during A/B update                              | ✅      |
| 13    | trident     | [verity.rs:185](../crates/trident/src/engine/storage/verity.rs#L185)                              | `open_verity_device_with_signature`              | ✅   | Validates signature file is NOT on ESP mount point (×2 at L185, L187)                   | ✅      |
| 14    | trident     | [image.rs:75](../crates/trident/src/engine/storage/image.rs#L75)                                  | `deploy_images`                                  | ✅   | Maps ESP filesystem in raw COSI storage mode                                            | ✅      |
| 15    | trident     | [uki.rs:62](../crates/trident/src/engine/boot/uki.rs#L62)                                         | `stage_uki_on_esp`                               | 🔗   | UKI staging path construction; caller has `ctx`                                         | ✅      |
| 16    | trident     | [uki.rs:148](../crates/trident/src/engine/boot/uki.rs#L148)                                       | `prepare_esp_for_uki`                            | 🔗   | ESP preparation for UKI; caller has `ctx`                                               | ✅      |
| 17    | trident     | [ab_update.rs:159](../crates/trident/src/engine/ab_update.rs#L159)                                | `finalize_update`                                | ✅   | `ctx` built locally at L144                                                             | ✅      |
| 18    | trident     | [clean_install.rs:310](../crates/trident/src/engine/clean_install.rs#L310)                        | `finalize_clean_install`                         | ✅   | `ctx` built locally at L283                                                             | ✅      |

---

### `ESP_RELATIVE_MOUNT_POINT_PATH`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L91) as `boot/efi`.

#### Usages

| Item# | Crate       | Location                                                                               | Function                            | Ctx | Notes                                                                    | Status |
| ----- | ----------- | -------------------------------------------------------------------------------------- | ----------------------------------- | --- | ------------------------------------------------------------------------ | ------ |
| 19    | trident_api | [constants.rs:95](../crates/trident_api/src/constants.rs#L95)                          | *(const)*                           | ➖   | Used to define `ESP_MOUNT_POINT_PATH`                                    | ☑️      |
| 20    | trident     | [init/offline/mod.rs:527](../crates/trident/src/init/offline/mod.rs#L527)              | `execute`                           | ❌   | Top-level offline-init command handler; no `EngineContext` in call chain | ✅      |
| 21    | trident     | [esp.rs:415](../crates/trident/src/subsystems/esp.rs#L415)                             | `copy_boot_files_for_uefi_fallback` | 🔗   | Caller `set_uefi_fallback_contents` has `ctx`                            | ✅      |
| 22    | trident     | [esp.rs:693](../crates/trident/src/subsystems/esp.rs#L693)                             | `generate_efi_bin_base_dir_path`    | ✅   | First param is `ctx: &EngineContext`                                     | ✅      |
| 23    | trident     | [install_index.rs:15](../crates/trident/src/engine/install_index.rs#L15)               | `next_install_index`                | ❌   | Pure utility; finds install index from path                              | ✅      |
| 24    | trident     | [manual_rollback/mod.rs:289](../crates/trident/src/engine/manual_rollback/mod.rs#L289) | `finalize_rollback`                 | ✅   | Param `engine_context: &EngineContext`                                   | ✅      |

---

### `ROOT_EFI_DIRECTORY`

Defined in [crates/trident_api/src/constants.rs](../crates/trident_api/src/constants.rs#L70) as `efi`.

#### Usages

| Item# | Crate       | Location                                                      | Function  | Ctx | Notes                                          | Status |
| ----- | ----------- | ------------------------------------------------------------- | --------- | --- | ---------------------------------------------- | ------ |
| 25    | trident_api | [constants.rs:91](../crates/trident_api/src/constants.rs#L91) | *(const)* | ➖   | Used to define `ESP_RELATIVE_MOUNT_POINT_PATH` | ☑️      |

---

## Test Usages

### `ESP_MOUNT_POINT_PATH`

| File                                                                         | Instances |
| ---------------------------------------------------------------------------- | --------- |
| crates/osutils/src/tabfile.rs                                                | 2         |
| crates/trident_api/src/config/host/storage/filesystem.rs                     | 1         |
| crates/trident_api/src/config/host/storage/mod.rs                            | 7         |
| crates/trident_api/src/config/host/storage/storage_graph/validation_tests.rs | 5         |
| crates/trident/src/engine/boot/grub.rs                                       | 1         |
| crates/trident/src/engine/boot/uki.rs                                        | 5         |
| crates/trident/src/engine/bootentries.rs                                     | 3         |
| crates/trident/src/engine/storage/encryption.rs                              | 3         |
| crates/trident/src/subsystems/storage/fstab.rs                               | 5         |
| crates/trident/src/subsystems/storage/osimage.rs                             | 2         |

### `ESP_RELATIVE_MOUNT_POINT_PATH`

| File                                 | Instances |
| ------------------------------------ | --------- |
| crates/trident/src/subsystems/esp.rs | 3         |
| crates/trident/src/engine/newroot.rs | 2         |

### `ROOT_EFI_DIRECTORY`

No test usages.

---

## Review Notes

1. These are just samples, it makes sense for these to grab the default constant.
2. This is the default ESP mount path used in the `fs_serde` deserialization
   module. It is the value used to detect whether a filesystem's mount point
   matches the default ESP path.
3. This is a top-level offline-init handler that constructs the ESP path from
   `ROOT_MOUNT_POINT_PATH` + `ESP_RELATIVE_MOUNT_POINT_PATH` before any
   `EngineContext` is created.
