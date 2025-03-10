use super::types::{BlkDevKind, BlkDevReferrerKind, FileSystemSourceKind};

impl BlkDevReferrerKind {
    /// Returns whether a referrer kind should be included in the public
    /// documentation
    pub fn document(&self) -> bool {
        true
    }
}

impl FileSystemSourceKind {
    /// Returns whether a file system source kind should be included in the public
    /// documentation
    pub fn document(&self) -> bool {
        true
    }
}

impl BlkDevKind {
    /// Returns whether a referrer kind flag should be included in the public
    /// documentation
    pub fn document(&self) -> bool {
        true
    }
}
