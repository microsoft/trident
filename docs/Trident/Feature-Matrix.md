---
sidebar_position: 5
---

# Feature Matrix

Legend:

- ✅: Fully supported.
- ☑️: In preview or partially supported.
- 🔜: Planned feature. Not implemented yet.
- ⚠️: Refer to relevant notes for details.
- ❌: Not supported.

## Servicing Features

| Category        | Feature                                 | Install | VM-Init | Update |
| --------------- | --------------------------------------- | ------- | ------- | ------ |
| 🚀 Runtime       | Native binary                           | ✅       | ✅       | ✅      |
| 🚀 Runtime       | Containerized                           | ✅       | ❌       | ✅      |
| ⚙️ Bootloader    | UEFI [1]                                | ✅       | ✅       | ✅      |
| ⚙️ Bootloader    | GPT partitioning                        | ✅       | ✅       | ✅      |
| ⚙️ Bootloader    | Grub2                                   | ✅       | ✅       | ✅      |
| ⚙️ Bootloader    | Systemd-boot                            | ☑️       | ☑️       | ☑️      |
| 🔄 Lifecycle     | Onboard system for updates              | ✅       | ✅       | ✅      |
| 🔄 Lifecycle     | Rollback (grub)                         | ✅       | ✅       | ✅      |
| 🔄 Lifecycle     | Rollback (systemd-boot/UKI)             | 🔜       | 🔜       | 🔜      |
| 🔏 Integrity     | Secure boot                             | ✅       | ✅       | ✅      |
| 🔏 Integrity     | UKI                                     | ☑️       | ☑️       | ☑️      |
| 🔏 Integrity     | Root verity (grub)                      | ⚠️[2]    | ⚠️[2]    | ⚠️[2]   |
| 🔏 Integrity     | Root verity (UKI)                       | ☑️       | ☑️       | ☑️      |
| 🔏 Integrity     | User verity (UKI)                       | ☑️       | ☑️       | ☑️      |
| 💽 Storage       | Block device creation                   | ✅       | 🔜       | ❌      |
| 💽 Storage       | Image streaming (local)                 | ✅       | 🔜       | ✅      |
| 💽 Storage       | Image streaming (HTTPS)                 | ✅       | 🔜       | ✅      |
| 💽 Storage       | Multiboot                               | ☑️       | ❌       | ✅[3]   |
| 💽 Storage       | Partition adoption                      | ☑️       | ❌       | ✅[3]   |
| 💽 Storage       | Software RAID                           | ✅       | ❌       | ✅[3]   |
| 💽 Storage       | ESP redundancy                          | ✅       | ❌       | ✅[3]   |
| 💽 Storage       | Encryption with secure boot PCR sealing | ✅       | 🔜       | ✅[3]   |
| 💽 Storage       | Encryption with OS PCR sealing          | 🔜[4]    | 🔜       | ✅[3]   |
| 📝 OS Config     | Network configuration                   | ✅       | ❌       | ✅      |
| 📝 OS Config     | Hostname configuration                  | ✅[5]    | ❌       | ✅[5]   |
| 📝 OS Config     | User configuration                      | ✅[5]    | ❌       | ✅[5]   |
| 📝 OS Config     | SSH configuration                       | ✅[5]    | ❌       | ✅[5]   |
| 📝 OS Config     | Initrd regeneration (grub)              | ✅       | ❌       | ✅      |
| 📝 OS Config     | Initrd regeneration (UKI)               | ❌       | ❌       | ❌      |
| 🛡️ Security      | SELinux Configuration                   | ✅       | ❌       | ✅      |
| 🪛 Customization | User provided-scripts                   | ✅       | ❌       | ✅      |
| 🛠️ Development   | Offline validation                      | ✅       | 🔜       | 🔜      |
| 🛠️ Development   | Debugging log                           | ✅       | ✅       | ✅      |

_Notes:_

- [1] Trident exclusively supports UEFI booting. BIOS booting is not supported.
- [2] Root verity is supported with grub, but support for this feature
  will be deprecated soon.
- [3] A system installed with these features can be updated, but the features
  themselves cannot be activated during an update.
- [4] Currently, only PCR 7 is supported. Sealing against other PCRs is
  planned for a future release.
- [5] These features cannot be used in conjunction with root verity.

## Out-of-Band Features

These are features that exist outside of the normal servicing flows in Trident.

| Category  | Feature      | Status | Notes                                                             |
| --------- | ------------ | ------ | ----------------------------------------------------------------- |
| 💽 Storage | RAID Rebuild | ✅      | Rebuild a software RAID array after a physical drive replacement. |