use std::{
    fs, io,
    process::{Command, Output},
};

pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/10-trident.yaml";

pub fn write(data: &str) -> io::Result<()> {
    fs::write(TRIDENT_NETPLAN_FILE, data)
}

pub fn apply() -> io::Result<Output> {
    Command::new("/usr/sbin/netplan").args(["apply"]).output()
}
