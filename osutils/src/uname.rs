use anyhow::{Context, Error};

use crate::dependencies::Dependency;

// Grab the kernel version using the `uname` command
pub fn kernel_release() -> Result<String, Error> {
    Dependency::Uname
        .cmd()
        .arg("-r")
        .output_and_check()
        .context("Failed to run uname -r")
}

#[cfg(test)]
mod tests {
    use crate::uname;
    #[test]
    fn test_kernel_release() {
        uname::kernel_release().unwrap();
    }
}
