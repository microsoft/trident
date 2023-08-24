use std::{
    collections::HashMap,
    fs, io,
    process::{Command, Output},
};

use anyhow::{Context, Error};

pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/99-trident.yaml";

pub fn write(data: &str) -> io::Result<()> {
    fs::write(TRIDENT_NETPLAN_FILE, data)
}

pub fn apply() -> io::Result<Output> {
    Command::new("/usr/sbin/netplan").args(["apply"]).output()
}

pub fn render_netplan_yaml(value: &serde_yaml::Value) -> Result<String, Error> {
    let final_map = HashMap::from([("network", value)]);

    serde_yaml::to_string(&final_map).context("failed to render netplan yaml")
}

#[cfg(test)]
mod tests {
    use super::render_netplan_yaml;
    use indoc::indoc;

    #[test]
    fn test_render_yaml() {
        let sample = indoc! {"
        ethernets:
          test:
            match:
              name: e*
        "};

        let expected = indoc! {"
        network:
          ethernets:
            test:
              match:
                name: e*
        "};

        let parsed =
            serde_yaml::from_str::<serde_yaml::Value>(sample).expect("Test yaml should be valid!");
        let out = render_netplan_yaml(&parsed).expect("failed to render test yaml");

        assert_eq!(out, expected);
    }
}
