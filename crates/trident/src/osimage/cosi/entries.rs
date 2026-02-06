use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Error};

/// Entry inside the COSI file.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(super) struct CosiEntry {
    pub offset: u64,
    pub size: u64,
}

impl CosiEntry {
    /// Returns the end offset of this entry, which is the offset plus the size.
    fn end(&self) -> u64 {
        self.offset + self.size
    }
}

/// A cache for collecting COSI entries as they are read. This allows us to keep
/// track of the last entry's end offset, which is necessary for determining
/// where the next unknown entry would start.
#[derive(Debug, Default, Clone)]
pub(super) struct CosiEntries {
    entries: HashMap<PathBuf, CosiEntry>,
    last_entry_end: u64,
}

impl CosiEntries {
    /// Registers a new entry in the COSI file. This will update the last entry
    /// end offset if necessary. It will ensure that there are no duplicate
    /// entries for the same path, as that would indicate a malformed COSI file.
    pub fn register(&mut self, path: impl AsRef<Path>, entry: CosiEntry) -> Result<(), Error> {
        self.last_entry_end = self.last_entry_end.max(entry.end());
        ensure!(
            self.entries
                .insert(path.as_ref().to_path_buf(), entry)
                .is_none(),
            "Found multiple entries in COSI file with path: {:?}",
            path.as_ref()
        );

        Ok(())
    }

    /// Retrieves an entry by its path.
    pub fn get(&self, path: impl AsRef<Path>) -> Option<&CosiEntry> {
        self.entries.get(path.as_ref())
    }

    /// Checks if an entry exists for the given path. Currently only used in
    /// tests to verify that entries are registered correctly.
    #[cfg(test)]
    pub fn contains_key(&self, path: impl AsRef<Path>) -> bool {
        self.entries.contains_key(path.as_ref())
    }

    /// Returns the offset where the next unknown entry would start, which is
    /// the end of the last known entry rounded up to the next 512-byte
    /// boundary.
    pub fn next_entry_offset(&self) -> u64 {
        self.last_entry_end.next_multiple_of(512)
    }

    /// Returns the total number of entries registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Allows collecting an iterator of `(PathBuf, CosiEntry)` tuples directly into
/// a `CosiEntries` instance, calculating the last entry end offset
/// automatically.
///
/// NOTE: FUNCTION MAY PANIC! THIS IS ONLY USED IN TESTS TO SIMPLIFY SETUP.
#[cfg(test)]
impl FromIterator<(PathBuf, CosiEntry)> for CosiEntries {
    fn from_iter<I: IntoIterator<Item = (PathBuf, CosiEntry)>>(iter: I) -> Self {
        let mut entries = CosiEntries::default();
        for (path, entry) in iter {
            entries
                .register(path, entry)
                .expect("Failed to register COSI entry from iterator");
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests [`CosiEntry::end`] for calculating the end offset of an entry.
    ///
    /// The end offset is simply the starting offset plus the size of the entry,
    /// representing the byte position immediately after the entry's content.
    #[test]
    fn test_cosi_entry_end() {
        let entry = CosiEntry {
            offset: 512,
            size: 1024,
        };
        assert_eq!(entry.end(), 1536);

        // Test with zero offset
        let entry = CosiEntry {
            offset: 0,
            size: 100,
        };
        assert_eq!(entry.end(), 100);

        // Test with zero size
        let entry = CosiEntry {
            offset: 512,
            size: 0,
        };
        assert_eq!(entry.end(), 512);
    }

    /// Tests [`CosiEntries::register`] for adding entries and tracking the last entry end.
    ///
    /// The register method should:
    /// 1. Add the entry to the internal map.
    /// 2. Update `last_entry_end` to track the furthest byte position seen.
    #[test]
    fn test_cosi_entries_register() {
        let mut entries = CosiEntries::default();

        // Register first entry
        entries
            .register(
                "file1.txt",
                CosiEntry {
                    offset: 0,
                    size: 100,
                },
            )
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("file1.txt"));

        // Register second entry - should update last_entry_end
        entries
            .register(
                "file2.txt",
                CosiEntry {
                    offset: 512,
                    size: 200,
                },
            )
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("file2.txt"));

        // Verify last_entry_end tracks the furthest position (512 + 200 = 712)
        // next_entry_offset rounds up to 512-byte boundary
        assert_eq!(entries.next_entry_offset(), 1024);
    }

    /// Tests [`CosiEntries::register`] when entries are added out of order.
    ///
    /// The `last_entry_end` should always reflect the maximum end position seen,
    /// regardless of the order entries are registered.
    #[test]
    fn test_cosi_entries_register_out_of_order() {
        let mut entries = CosiEntries::default();

        // Register a later entry first
        entries
            .register(
                "late.txt",
                CosiEntry {
                    offset: 2048,
                    size: 100,
                },
            )
            .unwrap();
        assert_eq!(entries.next_entry_offset(), 2560); // 2148 rounded up to 2560

        // Register an earlier entry - should NOT decrease last_entry_end
        entries
            .register(
                "early.txt",
                CosiEntry {
                    offset: 0,
                    size: 50,
                },
            )
            .unwrap();
        assert_eq!(entries.next_entry_offset(), 2560); // Still 2560
    }

    /// Tests [`CosiEntries::get`] for retrieving entries by path.
    ///
    /// Should return `Some(&CosiEntry)` for registered paths and `None` for
    /// unregistered paths.
    #[test]
    fn test_cosi_entries_get() {
        let mut entries = CosiEntries::default();

        let entry = CosiEntry {
            offset: 512,
            size: 256,
        };
        entries.register("test.txt", entry).unwrap();

        // Retrieve existing entry
        let retrieved = entries.get("test.txt");
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.offset, 512);
        assert_eq!(retrieved.size, 256);

        // Retrieve non-existent entry
        assert!(entries.get("nonexistent.txt").is_none());
    }

    /// Tests [`CosiEntries::get`] with nested paths.
    ///
    /// Paths with directory components should be stored and retrieved correctly.
    #[test]
    fn test_cosi_entries_get_nested_path() {
        let mut entries = CosiEntries::default();

        entries
            .register(
                "some/nested/path/file.txt",
                CosiEntry {
                    offset: 0,
                    size: 100,
                },
            )
            .unwrap();

        assert!(entries.get("some/nested/path/file.txt").is_some());
        assert!(entries.get("file.txt").is_none());
        assert!(entries.get("some/nested").is_none());
    }

    /// Tests [`CosiEntries::contains_key`] for checking entry existence.
    #[test]
    fn test_cosi_entries_contains_key() {
        let mut entries = CosiEntries::default();

        entries
            .register(
                "exists.txt",
                CosiEntry {
                    offset: 0,
                    size: 100,
                },
            )
            .unwrap();

        assert!(entries.contains_key("exists.txt"));
        assert!(!entries.contains_key("missing.txt"));
    }

    /// Tests [`CosiEntries::next_entry_offset`] for calculating the next valid offset.
    ///
    /// In tar archives, entries are aligned to 512-byte boundaries. This method
    /// returns the end of the last known entry, rounded up to the next 512-byte
    /// boundary, which is where the next entry header would begin.
    #[test]
    fn test_cosi_entries_next_entry_offset() {
        let mut entries = CosiEntries::default();

        // Empty entries should return 0
        assert_eq!(entries.next_entry_offset(), 0);

        // Entry ending exactly on boundary (512 + 512 = 1024)
        entries
            .register(
                "aligned.txt",
                CosiEntry {
                    offset: 512,
                    size: 512,
                },
            )
            .unwrap();
        assert_eq!(entries.next_entry_offset(), 1024);

        // Entry ending off-boundary should round up
        let mut entries = CosiEntries::default();
        entries
            .register(
                "unaligned.txt",
                CosiEntry {
                    offset: 512,
                    size: 100,
                },
            )
            .unwrap();
        // 512 + 100 = 612, rounded up to 1024
        assert_eq!(entries.next_entry_offset(), 1024);

        // Entry ending just past a boundary
        let mut entries = CosiEntries::default();
        entries
            .register(
                "test.txt",
                CosiEntry {
                    offset: 0,
                    size: 513,
                },
            )
            .unwrap();
        // 513 rounded up to 1024
        assert_eq!(entries.next_entry_offset(), 1024);
    }

    /// Tests [`CosiEntries::len`] for counting registered entries.
    #[test]
    fn test_cosi_entries_len() {
        let mut entries = CosiEntries::default();

        assert_eq!(entries.len(), 0);

        entries
            .register(
                "a.txt",
                CosiEntry {
                    offset: 0,
                    size: 10,
                },
            )
            .unwrap();
        assert_eq!(entries.len(), 1);

        entries
            .register(
                "b.txt",
                CosiEntry {
                    offset: 512,
                    size: 20,
                },
            )
            .unwrap();
        assert_eq!(entries.len(), 2);

        entries
            .register(
                "c.txt",
                CosiEntry {
                    offset: 1024,
                    size: 30,
                },
            )
            .unwrap();
        assert_eq!(entries.len(), 3);
    }

    /// Tests the [`FromIterator`] implementation for collecting into `CosiEntries`.
    ///
    /// This allows using `.collect()` on an iterator of `(PathBuf, CosiEntry)`
    /// tuples to produce a `CosiEntries` instance. The implementation should
    /// correctly calculate `last_entry_end` from all provided entries.
    #[test]
    fn test_cosi_entries_from_iterator() {
        let items = vec![
            (
                PathBuf::from("file1.txt"),
                CosiEntry {
                    offset: 0,
                    size: 100,
                },
            ),
            (
                PathBuf::from("file2.txt"),
                CosiEntry {
                    offset: 512,
                    size: 200,
                },
            ),
            (
                PathBuf::from("file3.txt"),
                CosiEntry {
                    offset: 1024,
                    size: 50,
                },
            ),
        ];

        let entries: CosiEntries = items.into_iter().collect();

        assert_eq!(entries.len(), 3);
        assert!(entries.contains_key("file1.txt"));
        assert!(entries.contains_key("file2.txt"));
        assert!(entries.contains_key("file3.txt"));

        // Verify last_entry_end is correctly calculated
        // Max end is file2: 512 + 200 = 712, but file3: 1024 + 50 = 1074
        // 1074 rounded up to 1536
        assert_eq!(entries.next_entry_offset(), 1536);
    }

    /// Tests [`FromIterator`] with an empty iterator.
    ///
    /// Collecting an empty iterator should produce a default `CosiEntries`.
    #[test]
    fn test_cosi_entries_from_iterator_empty() {
        let items: Vec<(PathBuf, CosiEntry)> = vec![];
        let entries: CosiEntries = items.into_iter().collect();

        assert_eq!(entries.len(), 0);
        assert_eq!(entries.next_entry_offset(), 0);
    }

    /// Tests that registering the same path twice results in an error.
    ///
    /// The `register` method uses `ensure!` to verify that no duplicate paths
    /// exist, returning an error if a path has already been registered.
    #[test]
    fn test_cosi_entries_register_duplicate() {
        let mut entries = CosiEntries::default();

        // First registration should succeed
        entries
            .register(
                "file.txt",
                CosiEntry {
                    offset: 0,
                    size: 100,
                },
            )
            .unwrap();

        // Second registration with the same path should fail
        let result = entries.register(
            "file.txt",
            CosiEntry {
                offset: 512,
                size: 200,
            },
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Found multiple entries in COSI file with path"));

        // Note: HashMap::insert replaces the value before returning the old one,
        // so after the error, the new entry is in the map (not the original).
        assert_eq!(entries.len(), 1);
        let entry = entries.get("file.txt").unwrap();
        assert_eq!(entry.offset, 512);
        assert_eq!(entry.size, 200);
    }
}
