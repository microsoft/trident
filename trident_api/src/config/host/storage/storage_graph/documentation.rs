use super::types::{BlkDevKind, BlkDevReferrerKind, FileSystemSourceKind};

impl BlkDevReferrerKind {
    /// Returns whether a referrer kind should be included in the public
    /// documentation
    pub fn document(&self) -> bool {
        !matches!(
            self,
            // None is a 'null' referrer and should not be documented
            BlkDevReferrerKind::None
                // These are internal and should not be documented
                | BlkDevReferrerKind::FileSystemOsImage
        )
    }
}

impl FileSystemSourceKind {
    /// Returns whether a file system source kind should be included in the public
    /// documentation
    pub fn document(&self) -> bool {
        !matches!(
            self,
            // These are internal and should not be documented
            FileSystemSourceKind::OsImage
        )
    }
}

impl BlkDevKind {
    /// Returns whether a referrer kind flag should be included in the public
    /// documentation
    pub fn document(&self) -> bool {
        true
    }
}
