use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::Dependency;

/// Device transport reported by udev.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Properties {
    /// `ID_BUS` as reported, or `None` when the property is absent.
    pub bus: Option<String>,
}

/// Reads the device's reported bus using `udevadm info`.
///
/// Command failures and malformed property lines return errors. Missing
/// `ID_BUS` is valid; other property names are ignored.
pub fn properties(path: &Path) -> Result<Properties, Error> {
    let output = Dependency::Udevadm
        .cmd()
        .args(["info", "--query=property", "--name"])
        .arg(path)
        .output_and_check()
        .with_context(|| format!("Failed to read udev properties for '{}'", path.display()))?;
    parse_properties(&output)
}

/// Extracts the bus value without filesystem access.
///
/// Every nonempty line must contain `=`. Repeated property names use the last
/// value.
fn parse_properties(output: &str) -> Result<Properties, Error> {
    let mut properties = Properties::default();
    for line in output.lines().filter(|line| !line.is_empty()) {
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("Invalid udev property: '{line}'"))?;
        if key == "ID_BUS" {
            properties.bus = Some(value.to_owned());
        }
    }
    Ok(properties)
}

pub fn settle() -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("settle")
        .run_and_check()
        .context("Failed settle udev setup")
}

pub fn trigger() -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("trigger")
        .run_and_check()
        .context("Failed trigger udev")
}

pub fn wait(path: &Path) -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("wait")
        .arg("--settle")
        .arg("--timeout=120")
        .arg(path)
        .run_and_check()
        .context("Failed wait udev")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bus_and_ignores_other_properties() {
        let properties = parse_properties("ID_BUS=usb\nUNUSED=value=with=equals\n").unwrap();
        assert_eq!(
            properties,
            Properties {
                bus: Some("usb".into()),
            }
        );
    }

    #[test]
    fn handles_missing_empty_and_repeated_properties() {
        assert_eq!(parse_properties("").unwrap(), Properties::default());
        assert_eq!(
            parse_properties("UNUSED=value\n").unwrap(),
            Properties::default()
        );
        assert_eq!(
            parse_properties("ID_BUS=usb\nID_BUS=\n").unwrap(),
            Properties {
                bus: Some(String::new()),
            }
        );
        assert_eq!(
            parse_properties("ID_BUS=value=with=equals\n")
                .unwrap()
                .bus
                .as_deref(),
            Some("value=with=equals")
        );
    }

    #[test]
    fn rejects_malformed_lines_even_for_unconsumed_properties() {
        for output in ["ID_BUS", "ID_BUS=usb\nBROKEN", "ID_BUS=usb\n \n"] {
            assert!(parse_properties(output).is_err(), "{output:?}");
        }
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_settle() {
        settle().unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_trigger() {
        trigger().unwrap();
        wait(Path::new("/dev/sda")).unwrap();
    }
}
