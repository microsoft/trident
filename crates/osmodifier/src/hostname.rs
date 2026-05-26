// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hostname management — writes /etc/hostname.

use std::fs;

use anyhow::{Context, Error};
use log::debug;

use crate::OsModifierContext;

const HOSTNAME_PATH: &str = "/etc/hostname";

/// Write the hostname to /etc/hostname.
pub fn update(ctx: &OsModifierContext, hostname: &str) -> Result<(), Error> {
    let path = ctx.path(HOSTNAME_PATH);
    debug!("Writing hostname '{}' to '{}'", hostname, path.display());
    fs::write(&path, hostname)
        .with_context(|| format!("Failed to write hostname to '{}'", path.display()))
}

#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use tempfile::tempdir;

    use pytest_gen::functional_test;

    use crate::OsModifierContext;

    #[functional_test(feature = "core")]
    fn test_update_hostname() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("etc")).unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        update(&ctx, "my-test-host").unwrap();

        let content = fs::read_to_string(tmp.path().join("etc/hostname")).unwrap();
        assert_eq!(content.trim(), "my-test-host");
    }

    #[functional_test(feature = "core")]
    fn test_update_hostname_overwrites() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("etc")).unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        update(&ctx, "first-host").unwrap();
        update(&ctx, "second-host").unwrap();

        let content = fs::read_to_string(tmp.path().join("etc/hostname")).unwrap();
        assert_eq!(content.trim(), "second-host");
    }
}
