//! This module contains helper functions for working with virtualized environments.

/// Does a best-effort check to determine whether we are running in a virtdeploy VM.
pub fn is_virtdeploy() -> bool {
    let mut index = 0;
    while let Ok(entry) =
        std::fs::read_to_string(format!("/sys/firmware/dmi/entries/11-{index}/raw"))
    {
        if entry.contains("virtdeploy:1") {
            return true;
        }
        index += 1;
    }

    false
}
