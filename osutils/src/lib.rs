pub mod chroot;
pub mod container;
pub mod exe;
pub mod files;
pub mod overlay;
pub mod scripts;
pub mod systemd;
pub mod udevadm;

pub(crate) mod crate_private {
    pub trait Sealed {}
}
