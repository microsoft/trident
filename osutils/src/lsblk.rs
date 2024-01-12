use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Error};
use log::warn;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::exe::RunAndCheck;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct LsBlkOutput {
    pub blockdevices: Vec<BlockDevice>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct BlockDevice {
    pub name: String,
    #[serde(rename = "partuuid")]
    pub part_uuid: Option<Uuid>,
    pub size: u64,
    #[serde(rename = "pkname")]
    pub parent_kernel_name: Option<PathBuf>,
    pub children: Option<Vec<BlockDevice>>,
}

pub fn run(device_path: impl AsRef<Path>) -> Result<Vec<BlockDevice>, Error> {
    let result = Command::new("lsblk")
        .arg("--json")
        .arg("--path")
        .arg(device_path.as_ref())
        .arg("--output-all")
        .arg("--bytes")
        .output_and_check()
        .context("Failed execute lsblk")?;

    let parsed = parse_lsblk_output(result.as_str());
    if parsed.is_err() {
        warn!("lsblk output: {}", result);
    }

    parsed
}

fn parse_lsblk_output(output: &str) -> Result<Vec<BlockDevice>, Error> {
    let parsed: LsBlkOutput =
        serde_json::from_str(output).context("Failed to parse lsblk output")?;

    Ok(parsed.blockdevices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lsblk_output() {
        let output = indoc::indoc!(
            r#"
            {
                "blockdevices": [
                    {
                        "name": "/dev/nvme0n1",
                        "kname": "/dev/nvme0n1",
                        "path": "/dev/nvme0n1",
                        "maj:min": "259:0",
                        "fsavail": null,
                        "fssize": null,
                        "fstype": null,
                        "fsused": null,
                        "fsuse%": null,
                        "fsroots": [
                            null
                        ],
                        "fsver": null,
                        "mountpoint": null,
                        "mountpoints": [
                            null
                        ],
                        "label": null,
                        "uuid": null,
                        "ptuuid": "fc6eb27a-2dfa-4acb-b5d6-7c5e1c821b71",
                        "pttype": "gpt",
                        "parttype": null,
                        "parttypename": null,
                        "partlabel": null,
                        "partuuid": null,
                        "partflags": null,
                        "ra": 128,
                        "ro": false,
                        "rm": false,
                        "hotplug": false,
                        "model": "SAMSUNG MZVPV512HDGL-000H1",
                        "serial": "S27FNYAH407000",
                        "size": 512110190592,
                        "state": "live",
                        "owner": "root",
                        "group": "disk",
                        "mode": "brw-rw----",
                        "alignment": 0,
                        "min-io": 512,
                        "opt-io": 0,
                        "phy-sec": 512,
                        "log-sec": 512,
                        "rota": false,
                        "sched": "none",
                        "rq-size": 1023,
                        "type": "disk",
                        "disc-aln": 0,
                        "disc-gran": 512,
                        "disc-max": 2199023255040,
                        "disc-zero": false,
                        "wsame": 0,
                        "wwn": "eui.002538646100e442",
                        "rand": false,
                        "pkname": null,
                        "hctl": null,
                        "tran": "nvme",
                        "subsystems": "block:nvme:pci",
                        "rev": null,
                        "vendor": null,
                        "zoned": "none",
                        "dax": false,
                        "children": [
                            {
                            "name": "/dev/nvme0n1p1",
                            "kname": "/dev/nvme0n1p1",
                            "path": "/dev/nvme0n1p1",
                            "maj:min": "259:1",
                            "fsavail": "529436672",
                            "fssize": "535805952",
                            "fstype": "vfat",
                            "fsused": "6369280",
                            "fsuse%": "1%",
                            "fsroots": [
                                "/"
                            ],
                            "fsver": "FAT32",
                            "mountpoint": "/boot/efi",
                            "mountpoints": [
                                "/boot/efi"
                            ],
                            "label": null,
                            "uuid": "84A0-088E",
                            "ptuuid": "fc6eb27a-2dfa-4acb-b5d6-7c5e1c821b71",
                            "pttype": "gpt",
                            "parttype": "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
                            "parttypename": "EFI System",
                            "partlabel": "EFI System Partition",
                            "partuuid": "b46b76eb-b2f9-441a-9686-8b24fa2b2161",
                            "partflags": null,
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 536870912,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": false,
                            "sched": "none",
                            "rq-size": 1023,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2199023255040,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": "eui.002538646100e442",
                            "rand": false,
                            "pkname": "/dev/nvme0n1",
                            "hctl": null,
                            "tran": "nvme",
                            "subsystems": "block:nvme:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            },{
                            "name": "/dev/nvme0n1p2",
                            "kname": "/dev/nvme0n1p2",
                            "path": "/dev/nvme0n1p2",
                            "maj:min": "259:2",
                            "fsavail": "60132933632",
                            "fssize": "502392610816",
                            "fstype": "ext4",
                            "fsused": "416664305664",
                            "fsuse%": "83%",
                            "fsroots": [
                                "/usr/share/hunspell", "/"
                            ],
                            "fsver": "1.0",
                            "mountpoint": "/",
                            "mountpoints": [
                                "/var/snap/firefox/common/host-hunspell", "/"
                            ],
                            "label": null,
                            "uuid": "f4c40183-0a2d-4d97-b71e-25a4043ce01f",
                            "ptuuid": "fc6eb27a-2dfa-4acb-b5d6-7c5e1c821b71",
                            "pttype": "gpt",
                            "parttype": "0fc63daf-8483-4772-8e79-3d69d8477de4",
                            "parttypename": "Linux filesystem",
                            "partlabel": null,
                            "partuuid": "af002b41-3dbe-4044-82d2-f0560ef58b7a",
                            "partflags": null,
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 511571918848,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": false,
                            "sched": "none",
                            "rq-size": 1023,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2199023255040,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": "eui.002538646100e442",
                            "rand": false,
                            "pkname": "/dev/nvme0n1",
                            "hctl": null,
                            "tran": "nvme",
                            "subsystems": "block:nvme:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            }
                        ]
                    }
                ]
            }
        "#
        );
        let expected_block_device_list = vec![BlockDevice {
            name: "/dev/nvme0n1".into(),
            part_uuid: None,
            size: 512110190592,
            parent_kernel_name: None,
            children: Some(vec![
                BlockDevice {
                    name: "/dev/nvme0n1p1".into(),
                    part_uuid: Some(
                        Uuid::parse_str("b46b76eb-b2f9-441a-9686-8b24fa2b2161").unwrap(),
                    ),
                    size: 536870912,
                    parent_kernel_name: Some(PathBuf::from("/dev/nvme0n1")),
                    children: None,
                },
                BlockDevice {
                    name: "/dev/nvme0n1p2".into(),
                    part_uuid: Some(
                        Uuid::parse_str("af002b41-3dbe-4044-82d2-f0560ef58b7a").unwrap(),
                    ),
                    size: 511571918848,
                    parent_kernel_name: Some(PathBuf::from("/dev/nvme0n1")),
                    children: None,
                },
            ]),
        }];
        let block_device_list = parse_lsblk_output(output).unwrap();
        assert_eq!(block_device_list, expected_block_device_list);

        assert!(parse_lsblk_output("bad output").is_err());
    }
}

#[cfg(feature = "functional-tests")]
mod functional_tests {
    use pytest_gen::pytest;

    #[cfg(test)]
    use super::*;

    #[pytest(feature = "helpers")]
    fn test_run_success() {
        let block_device_list = super::run(Path::new("/dev/sda")).unwrap();

        assert_eq!(block_device_list.len(), 1);
        assert_eq!(block_device_list[0].name, "/dev/sda");
        assert_eq!(block_device_list[0].children.as_ref().unwrap().len(), 5);
    }

    #[pytest(feature = "helpers", negative = true)]
    fn test_run_fail_on_non_block_file() {
        assert_eq!(super::run(Path::new("/dev/null")).unwrap_err().root_cause().to_string(), "Process output:\nstdout:\n{\n   \"blockdevices\": [\n\n   ]\n}\n\n\nstderr:\nlsblk: /dev/null: not a block device\n\n");
    }

    #[pytest(feature = "helpers", negative = true)]
    fn test_run_fail_on_missing_file() {
        assert_eq!(super::run(Path::new("/dev/does-not-exist")).unwrap_err().root_cause().to_string(), "Process output:\nstdout:\n{\n   \"blockdevices\": [\n\n   ]\n}\n\n\nstderr:\nlsblk: /dev/does-not-exist: not a block device\n\n");
    }
}
