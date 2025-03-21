use std::{
    borrow::Cow,
    fs,
    io::Write,
    iter,
    path::{Path, PathBuf},
    time::Instant,
};

use const_format::formatcp;
use rand::{
    distr::{Bernoulli, Distribution, Uniform},
    Rng,
};

use crate::{
    dependencies::Dependency,
    files,
    filesystems::{MkfsFileSystemType, MountFileSystemType},
    mkfs,
    repart::{RepartEmptyMode, SystemdRepartInvoker},
    udevadm, veritysetup,
};

use super::{
    repart::{self, TEST_DISK_DEVICE_PATH},
    tmp_mount,
};

pub struct VerityGuard<'a> {
    pub device_name: &'a str,
}

impl Drop for VerityGuard<'_> {
    fn drop(&mut self) {
        // Try to close, but ignore errors
        veritysetup::close(self.device_name).ok();
    }
}

/// Generates a random alphanumeric string of length 32.
fn gen_random_string() -> String {
    let mut rng = rand::rng();
    iter::repeat(())
        .map(|()| rng.sample(rand::distr::Alphanumeric) as char)
        .take(32)
        .collect()
}

pub struct TestVerityVolume {
    pub data_volume: PathBuf,
    pub hash_volume: PathBuf,
    pub root_hash: String,
    pub file_list: Vec<PathBuf>,
}

impl TestVerityVolume {
    /// Opens the verity volume and returns a VerityGuard.
    pub fn open_verity<'a>(&self, name: &'a str) -> VerityGuard<'a> {
        veritysetup::open(&self.data_volume, name, &self.hash_volume, &self.root_hash).unwrap();

        VerityGuard { device_name: name }
    }
}

/// Generates an ext4 filesystem on the data device and populates it with random data.
/// Then, creates a verity hash image on the hash device.
///
/// Returns the root hash of the verity volume.
fn generate_and_write_verity_images(
    data_dev: impl AsRef<Path>,
    hash_dev: impl AsRef<Path>,
) -> TestVerityVolume {
    let start_time = Instant::now();

    let log = |msg: &str| {
        println!(
            "[{:2.2}s] {msg}",
            start_time.elapsed().as_secs_f64(),
            msg = msg
        )
    };

    // Create an ext4 filesystem on the data device. Set ext4 block size to 4096
    // as discussed in https://github.com/systemd/systemd/issues/11123.
    Dependency::Mkfs
        .cmd()
        .arg("--type")
        .arg(MkfsFileSystemType::Ext4.name())
        .arg("-b")
        .arg("4096")
        .arg(data_dev.as_ref())
        .run_and_check()
        .unwrap();
    log("Created ext4 filesystem on data device");

    // Create a list to keep track of the files created
    let mut file_list = Vec::new();

    // Just so that the FS is populated with something and we can validate that
    // the contents are preserved, create several random files in random
    // locations in the filesystem. To make something random-ish, but also
    // resemble a real filesystem, we do a random walk.

    // Create a Bernoulli distribution with p = 0.5. Unwrap is safe here because
    // the probability is hard-coded.
    let walk_dist = Bernoulli::new(0.5).unwrap();
    // Create a random number generator for how many files to create in each
    // directory. Unwrap is safe here because the range is hard-coded.
    let file_dist = Uniform::new(0, 10).unwrap();
    // Create a random number generator for the random walk.
    let mut rng = rand::rng();
    // Set up a stack to keep track of the current directory.
    let mut dir_stack: Vec<String> = Vec::new();
    // Temporary mount the data device and populate it with random data
    tmp_mount::mount(
        data_dev.as_ref(),
        MountFileSystemType::Ext4,
        &[],
        |mount_dir| {
            // Create this many files
            for _ in 0..50 {
                // Random walk!
                if walk_dist.sample(&mut rng) {
                    // Go up a directory
                    dir_stack.pop();
                } else {
                    // Go down a directory
                    dir_stack.push(gen_random_string());
                }

                let dir_path = mount_dir.join(dir_stack.join("/"));

                // Create the directory
                fs::create_dir_all(&dir_path).unwrap();

                // Create some files in the directory
                for _ in 0..file_dist.sample(&mut rng) {
                    let file_path = dir_path.join(gen_random_string());
                    let mut file = std::fs::File::create(&file_path).unwrap();
                    file.write_all(&[0; 1024]).unwrap();

                    file_list.push(file_path.strip_prefix(mount_dir).unwrap().to_path_buf());
                }
            }
        },
    );

    log("Populated filesystem with random data");

    // Create a verity hash image and return the root hash
    let root_hash = veritysetup_format(data_dev.as_ref(), hash_dev.as_ref());

    log("Created verity hash image");

    TestVerityVolume {
        data_volume: data_dev.as_ref().to_path_buf(),
        hash_volume: hash_dev.as_ref().to_path_buf(),
        root_hash,
        file_list,
    }
}

/// Sets up verity volumes on the test disk.
///
/// Returns a TestVerityVolume struct containing the data and hash volumes, as well as the root hash.
///
/// Currently:
///
/// - `/dev/<testdisk>3`: Data
/// - `/dev/<testdisk>2`: Hash
pub fn setup_verity_volumes() -> TestVerityVolume {
    let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
        .with_partition_entries(repart::generate_partition_definition_boot_root_verity());

    repart.execute().unwrap();
    udevadm::settle().unwrap();

    generate_and_write_verity_images(
        formatcp!("{TEST_DISK_DEVICE_PATH}3"),
        formatcp!("{TEST_DISK_DEVICE_PATH}2"),
    )
}

/// Sets up mock root verity volumes on the test disk.
///
/// Returns a tuple containing the boot device and the mock verity volume info.
///
/// Currently:
///
/// - `/dev/<testdisk>1`: Boot
/// - `/dev/<testdisk>3`: Data
/// - `/dev/<testdisk>2`: Hash
pub fn setup_verity_volumes_with_boot() -> (PathBuf, TestVerityVolume) {
    let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
        .with_partition_entries(repart::generate_partition_definition_boot_root_verity());

    repart.execute().unwrap();
    udevadm::settle().unwrap();

    let boot_device = PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1"));
    let data_volume = formatcp!("{TEST_DISK_DEVICE_PATH}3");
    let hash_volume = formatcp!("{TEST_DISK_DEVICE_PATH}2");

    let verity_vol = generate_and_write_verity_images(data_volume, hash_volume);

    setup_fake_grub_config(
        &boot_device,
        data_volume,
        hash_volume,
        &verity_vol.root_hash,
    );

    (boot_device, verity_vol)
}

/// Sets up a root verity boot partition on the test disk.
///
/// It doesn't actually set up the verity stuff, just creates a partition with
/// an ext4 filesystem that only contains grub2/grub.cfg.
///
/// Returns a tuple containing the boot device and the mock verity volume info.
pub fn setup_root_verity_boot_partition() -> (PathBuf, TestVerityVolume) {
    let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
        .with_partition_entries(repart::generate_partition_definition_boot_root_verity());

    repart.execute().unwrap();
    udevadm::settle().unwrap();

    let boot_device = PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1"));
    let data_volume = formatcp!("{TEST_DISK_DEVICE_PATH}3");
    let hash_volume = formatcp!("{TEST_DISK_DEVICE_PATH}2");

    let root_hash = gen_random_string();

    setup_fake_grub_config(&boot_device, data_volume, hash_volume, &root_hash);

    (
        boot_device,
        TestVerityVolume {
            data_volume: PathBuf::from(data_volume),
            hash_volume: PathBuf::from(hash_volume),
            root_hash,
            file_list: Vec::new(),
        },
    )
}

/// Sets up a fake grub config with root verity in a fake boot volume
fn setup_fake_grub_config(
    boot_device: impl AsRef<Path>,
    data_device: impl AsRef<Path>,
    hash_device: impl AsRef<Path>,
    roothash: &str,
) {
    mkfs::run(boot_device.as_ref(), MkfsFileSystemType::Ext4).unwrap();

    let kernel_cmd = [
        Cow::from("systemd.verity=1"),
        Cow::from("root=/dev/mapper/root"),
        Cow::from("ro"),
        Cow::from(format!("roothash={roothash}")),
        Cow::from(format!(
            "systemd.verity_root_data={}",
            data_device.as_ref().display()
        )),
        Cow::from(format!(
            "systemd.verity_root_hash={}",
            hash_device.as_ref().display()
        )),
    ];

    let grub_cfg = include_str!("../test_files/grub.cfg.template")
        .replace("%%KERNELCMDLINE%%", &kernel_cmd.join(" "));

    tmp_mount::mount(&boot_device, MountFileSystemType::Ext4, &[], |mount_dir| {
        let mut file = files::create_file(mount_dir.join("grub2/grub.cfg")).unwrap();
        file.write_all(grub_cfg.as_bytes()).unwrap();
    });
}

/// Runs `veritysetup format` on the given data and hash devices and returns the roothash.
fn veritysetup_format(data_dev: impl AsRef<Path>, hash_dev: impl AsRef<Path>) -> String {
    let out = Dependency::Veritysetup
        .cmd()
        .arg("format")
        .arg(data_dev.as_ref())
        .arg(hash_dev.as_ref())
        .output_and_check()
        .unwrap();

    out.lines()
        .skip(1)
        .filter_map(|line| {
            let mut splits = line.split(':');
            let key = splits.next()?;
            let value = splits.next()?;
            Some((key, value))
        })
        .find(|(key, _)| *key == "Root hash")
        .unwrap()
        .1
        .trim()
        .to_owned()
}
