use std::{io::Write, path::Path};

use anyhow::{bail, Context, Error};

use crate::{
    files,
    osrelease::{AzureLinuxRelease, OS_RELEASE_PATH},
    path,
};

/// Azure Linux 2.0 sample os-release file.
const AZURE_LINUX_2_OS_RELEASE: &str = indoc::indoc! {
    r#"
    NAME="Common Base Linux Mariner"
    VERSION="2.0.20240609"
    ID=mariner
    VERSION_ID="2.0"
    PRETTY_NAME="CBL-Mariner/Linux"
    ANSI_COLOR="1;34"
    HOME_URL="https://aka.ms/cbl-mariner"
    BUG_REPORT_URL="https://aka.ms/cbl-mariner"
    SUPPORT_URL="https://aka.ms/cbl-mariner"
    "#,
};

/// Azure Linux 3.0 sample os-release file.
const AZURE_LINUX_3_OS_RELEASE: &str = indoc::indoc! {
    r#"
    NAME="Microsoft Azure Linux"
    VERSION="3.0.20240609"
    ID=azurelinux
    VERSION_ID="3.0"
    PRETTY_NAME="Microsoft Azure Linux 3.0"
    ANSI_COLOR="1;34"
    HOME_URL="https://aka.ms/azurelinux"
    BUG_REPORT_URL="https://aka.ms/azurelinux"
    SUPPORT_URL="https://aka.ms/azurelinux"
    "#,
};

/// Creates a mock /etc/os-release file with the given Azure Linux release.
pub fn make_mock_os_release(root_path: &Path, azl_release: AzureLinuxRelease) -> Result<(), Error> {
    let os_release_content = match azl_release {
        AzureLinuxRelease::AzL2 => AZURE_LINUX_2_OS_RELEASE,
        AzureLinuxRelease::AzL3 => AZURE_LINUX_3_OS_RELEASE,
        AzureLinuxRelease::Other => bail!("Unsupported Azure Linux release 'other'"),
    };

    let os_release_path = path::join_relative(root_path, OS_RELEASE_PATH);
    let mut file = files::create_file(&os_release_path).with_context(|| {
        format!(
            "Failed to create os-release file at '{}'",
            os_release_path.display()
        )
    })?;

    file.write_all(os_release_content.as_bytes())
        .with_context(|| {
            format!(
                "Failed to write os-release file to '{}'",
                os_release_path.display(),
            )
        })
}
