pub mod exe;
pub mod files;
pub mod scripts;

pub(crate) mod crate_private {
    pub trait Sealed {}
}
