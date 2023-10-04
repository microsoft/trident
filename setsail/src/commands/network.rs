use std::{
    hash::Hash,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use clap::{ArgGroup, Parser, ValueEnum};

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::{misc::KeyValList, HandleCommand};

/// The kickstart network command
///
/// Mimics <https://pykickstart.readthedocs.io/en/latest/kickstart-docs.html#network>
#[derive(Parser, Debug, Clone)]
#[clap(group(ArgGroup::new("min").required(true).multiple(true)))]
#[command(name = "network")]
pub struct Network {
    #[clap(skip)]
    pub line: KSLine,

    #[clap(skip)]
    pub device_type: DeviceType,

    /// Setup IPv4 on this device
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    #[arg(default_value = "dhcp")]
    #[arg(requires_if("static", "ip"))]
    #[arg(requires_if("static", "netmask"))]
    #[arg(verbatim_doc_comment)]
    pub bootproto: BootProto,
    // Note: Kickstart also supports bootproto=link, but we don't have a good way to support that
    /// Configure a device
    ///
    /// Possible values:
    ///
    /// - `<NAME>`: The name of the device to configure
    /// - `<MAC>`: The MAC address of the device to configure
    #[arg(long)]
    #[arg(group = "min")]
    #[arg(verbatim_doc_comment)]
    pub device: Option<DeviceReference>,

    /// Set the hostname of the system
    #[arg(long)]
    #[arg(group = "min")]
    pub hostname: Option<String>,

    /// Set up the gateway for this device
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    pub gateway: Option<Ipv4Addr>,

    /// Set up IPv4 on this device
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    pub ip: Option<Ipv4Addr>,

    /// Set up the netmask for this device
    ///
    /// Requires: `--device`
    ///
    /// Format: IP format: X.X.X.X
    ///
    /// Example: `255.255.255.0`
    #[arg(long, requires = "device")]
    pub netmask: Option<Ipv4Netmask>,

    /// Set up a specific MTU for this device
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    pub mtu: Option<u16>,

    /// Set up the nameservers for this device
    ///
    /// Requires: `--device`
    ///
    /// Format: comma-separated list of IP addresses
    #[arg(long, requires = "device")]
    #[arg(value_delimiter = ',')]
    pub nameserver: Vec<IpAddr>,

    /// Disables all DNS on this device, including DHCP-provided DNS
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    #[arg(conflicts_with = "nameserver")]
    pub nodns: bool,

    // (separator to appease cargo fmt)
    /// Disable all IPv4 on this device
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    pub noipv4: bool,

    /// Disable all IPv6 on this device
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    pub noipv6: bool,

    /// Set up this device as a bond with the following members
    ///
    /// Requires: `--device`
    ///
    /// Format: comma-separated list of device names
    #[arg(long)]
    #[arg(value_delimiter = ',', group = "type")]
    pub bondslaves: Vec<String>,

    /// Set up this device as a bond with the following options
    ///
    /// Requires: `--device`, `--bondslaves`
    ///
    /// Format: comma-separated list of key=value pairs, if a value contains a comma, semicolons should be used instead
    ///
    /// Example: `mode=active-backup,miimon=100`
    ///
    /// Supported options:
    ///
    /// - `ad_select`: (string)
    ///   - `stable` or `0`
    ///   - `bandwidth` or `1`
    ///   - `count` or `2`
    /// - `all_slaves_active`: (string)
    ///   - `dropped` or `0`
    ///   - `delivered` or `1`
    /// - `arp_all_targets`: (string)
    ///   - `any` or `0`
    ///   - `all` or `1`
    /// - `arp_interval`: (u32)
    /// - `arp_ip_target`: (string) comma-separated list of IP addresses
    /// - `arp_validate`: (string)
    ///   - `none` or `0`
    ///   - `active` or `1`
    ///   - `backup` or `2`
    ///   - `all` or `3`
    /// - `downdelay` (u32)
    /// - `fail_over_mac`: (string)
    ///   - `none` or `0`
    ///   - `active` or `1`
    ///   - `follow` or `2`
    /// - `lacp_rate`: (string)
    ///   - `fast` or `1`
    ///   - `slow` or `0`
    /// - `lp_interval`: (u32)
    /// - `miimon`: (u32)
    /// - `min_links`: (u16)
    /// - `mode`: (string)
    ///   - `balance-rr` or `0`
    ///   - `active-backup` or `1`
    ///   - `balance-xor` or `2`
    ///   - `broadcast` or `3`
    ///   - `802.3ad` or `4`
    ///   - `balance-tlb` or `5`
    ///   - `balance-alb` or `6`
    /// - `num_grat_arp`: (u8)
    /// - `packets_per_slave`: (u32)
    /// - `primary`: (string)
    /// - `primary_reselect`: (string)
    ///   - `always` or `0`
    ///   - `better` or `1`
    ///   - `failure` or `2`
    /// - `resend_igmp`: (u8)
    /// - `updelay`: (u32)
    /// - `xmit_hash_policy`: (string)
    ///   - `layer2`
    ///   - ~~`layer2+3`~~ **(Not currently supported)**
    ///   - `layer3+4`
    ///   - `encap2+3`
    ///   - `encap3+4`
    ///
    /// See <https://www.kernel.org/doc/Documentation/networking/bonding.txt> for more information
    ///
    /// The values are ultimately parsed by Netplan, so see
    /// <https://netplan.readthedocs.io/en/stable/netplan-yaml/#properties-for-device-type-bonds>
    /// for more information.
    ///
    /// Note: the numeric equivalents to the names defined in the kernel docs are also accepted
    #[arg(long, requires = "device")]
    #[arg(requires = "bondslaves")]
    #[arg(default_value = "")]
    #[arg(verbatim_doc_comment)]
    pub bondopts: KeyValList,

    /// Set up this device as a bridge with the following members
    ///
    /// Requires: `--device`
    ///
    /// Format: comma-separated list of device names
    #[arg(long, requires = "device")]
    #[arg(value_delimiter = ',', group = "type")]
    pub bridgeslaves: Vec<String>,

    /// Set up this device as a bridge with the following options
    ///
    /// Requires: `--device`, `--bridgeslaves`
    ///
    /// Format: comma-separated list of key=value pairs, if a value contains a comma, semicolons should be used instead
    ///
    /// Example: `stp=on,ageing-time=20`
    ///
    /// Supported options:
    ///
    /// - `stp`: (string)
    ///   - `0` or `t` or `true`  or `y` or `yes` or `off`
    ///   - `1` or `f` or `false` or `n` or `no`  or `on`
    /// - `priority`: (u32)
    /// - `forward-delay`: (u32)
    /// - `hello-time`: (u32)
    /// - `max-age`: (u32)
    /// - `ageing-time`: (u32)
    ///
    /// See <https://netplan.readthedocs.io/en/stable/netplan-yaml/#properties-for-device-type-bridges>
    /// for more information.
    #[arg(long, requires = "device")]
    #[arg(requires = "bridgeslaves")]
    #[arg(default_value = "")]
    #[arg(verbatim_doc_comment)]
    pub bridgeopts: KeyValList,

    /// Set up IPv6 on this device
    ///
    /// Requires: `--device`
    ///
    /// Options:
    ///
    /// - `auto`: Use the default IPv6 configuration for the system (link-local address)
    /// - `dhcp`: Use DHCPv6 to obtain an address
    /// - `<ipv6>/<prefix>`: Use the specified static IPv6 address and prefix
    #[arg(long, requires = "device")]
    #[arg(default_value = "auto")]
    #[arg(verbatim_doc_comment)]
    pub ipv6: Ipv6Config,

    /// Set up the gateway for this device
    ///
    /// Requires: `--device`
    ///
    /// Format: IPv6
    #[arg(long, requires = "device")]
    pub ipv6gateway: Option<Ipv6Addr>,

    /// Whether to activate this device on the provisioning environment
    ///
    /// Requires: `--device`
    ///
    /// The first device specified in the is activated by default
    #[arg(long, requires = "device")]
    #[arg(group = "activation")]
    pub activate: bool,

    /// Block this device from being activated in the provisioning environment
    ///
    /// Requires: `--device`
    #[arg(long = "no-activate", requires = "device")]
    #[arg(group = "activation")]
    pub no_activate: bool,

    /// Set up this device as a VLAN with the following ID
    ///
    /// Requires: `--device`
    #[arg(long, requires = "device")]
    pub vlanid: Option<u16>,

    /// When setting up a VLAN, set the interface name to this instead of the autogenerated name
    ///
    /// If the name contains a dot (.), it must take the form of `NAME.ID`. The NAME is
    /// arbitrary, but the ID must be the VLAN ID. For example: `em1.171` or `my-vlan.171`.
    /// Names starting with vlan must take the form of `vlanID` - for example: `vlan171`.
    ///
    /// Requires: `--device`, `--vlanid`
    ///
    /// Format: NAME, NAME.ID or vlanID
    ///
    /// Default: device.ID
    #[arg(long, requires = "device")]
    #[arg(requires = "vlanid")]
    pub interfacename: Option<String>,

    /// Set up the IPv4 DNS search domains for this device
    ///
    /// Requires: `--device`
    ///
    /// Format: comma-separated list of IPv4 addresses
    #[arg(long = "ipv4-dns-search", requires = "device")]
    #[arg(value_delimiter = ',', conflicts_with = "nodns")]
    pub ipv4_dns_search: Vec<Ipv4Addr>,

    /// Set up the IPv6 DNS search domains for this device
    ///
    /// Requires: `--device`
    ///
    /// Format: comma-separated list of IPv6 addresses
    #[arg(long = "ipv6-dns-search", requires = "device")]
    #[arg(value_delimiter = ',', conflicts_with = "nodns")]
    pub ipv6_dns_search: Vec<Ipv6Addr>,

    /// Ignore the IPv4 DNS servers provided by DHCP
    ///
    /// Requires: `--device`
    #[arg(long = "ipv4-ignore-auto-dns", requires = "device")]
    pub ipv4_ignore_auto_dns: bool,

    /// Ignore the IPv6 DNS servers provided by DHCP
    ///
    /// Requires: `--device`
    #[arg(long = "ipv6-ignore-auto-dns", requires = "device")]
    pub ipv6_ignore_auto_dns: bool,
    // Unsupported fields:

    // Wi-Fi fields:
    // #[arg(long, requires = "device")]
    // pub essid: String,
    // #[arg(long, requires = "device")]
    // pub wepkey: String,
    // #[arg(long, requires = "device")]
    // pub wpakey: String,

    // // Unsupported: Netplan does support a similar setting called `activation-mode`
    // but it is very recent and may not translate 1:1
    // #[arg(long, requires = "device")]
    // #[arg(default_value = "true")]
    // pub onboot: bool

    // // Unsupported: There isn't a good 1:1 mapping for this
    // #[arg(long, requires = "device")]
    // pub notksdevice: bool,

    // // Unsupported: not obvious way to support this
    // #[arg(long, requires = "device")]
    // pub dhcpclass: Option<String>,

    // // Unsupported: not obvious way to support this
    // #[arg(long, requires = "device")]
    // pub ethtool: Option<String>,

    // // Unsupported: not obvious way to support this
    // #[arg(long, requires = "device")]
    // pub onboot: bool,

    // // Unsupported: not obvious way to support this
    // #[arg(long, requires = "device")]
    // pub notksdevice: bool,

    // // Unsupported: not obvious way to support this
    // #[arg(long, requires = "device")]
    // pub bindto: Option<ValueEnum_Mac>,

    // // Unsupported: netplan does not support teaming
    // #[arg(long, requires = "device")]
    // #[arg(value_delimiter = ',', group = "type")]
    // pub teamslaves: Vec<String>,

    // // Unsupported: netplan does not support teaming
    // #[arg(long, requires = "device")]
    // #[arg(requires = "bridgeslaves")]
    // #[arg(default_value = "")]
    // pub teamconfig: Option<JSONString>,
}

#[derive(Debug, ValueEnum, Default, Clone, Copy)]
pub enum BootProto {
    /// Use DHCP to obtain an address
    #[default]
    Dhcp,
    /// Use the specified static IPv4 address and netmask, requires: `--ip` and `--netmask`
    Static,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum DeviceReference {
    /// The name of the device to configure
    Name(String),
    /// The MAC address of the device to configure
    Mac(MacAddress),
    // TODO: figure out how to support link
    //Link,
}

impl From<&str> for DeviceReference {
    fn from(value: &str) -> Self {
        /*if value == "link" {
            Self::Link
        } else*/
        if let Some(mac) = MacAddress::new_from_str(value) {
            Self::Mac(mac)
        } else {
            Self::Name(value.to_string())
        }
    }
}

impl std::fmt::Display for DeviceReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceReference::Name(name) => write!(f, "{}", name),
            DeviceReference::Mac(mac) => write!(f, "{}", mac),
            // DeviceReference::Link => write!(f, "link"),
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct MacAddress {
    pub address: [u8; 6],
}

impl MacAddress {
    fn new_from_str(value: &str) -> Option<Self> {
        let mut address = [0u8; 6];
        let parts: Vec<&str> = value.split(':').collect();
        if parts.len() != 6 {
            return None;
        }

        for (i, part) in parts.iter().enumerate() {
            address[i] = u8::from_str_radix(part, 16).ok()?;
        }

        Some(Self { address })
    }
}

impl std::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.address[0],
            self.address[1],
            self.address[2],
            self.address[3],
            self.address[4],
            self.address[5],
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Ipv6Config {
    Auto,
    Dhcp,
    Static(Ipv6Address),
}

impl std::str::FromStr for Ipv6Config {
    type Err = Box<dyn std::error::Error + Send + Sync>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "auto" => Self::Auto,
            "dhcp" => Self::Dhcp,
            _ => Self::Static(Ipv6Address::from_str(s)?),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ipv6Address {
    pub address: Ipv6Addr,
    pub prefix: u8,
}

impl FromStr for Ipv6Address {
    type Err = Box<dyn std::error::Error + Send + Sync>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('/');
        Ok(Self {
            // Note, we should never hit "empty iterator" error because split is guaranteed to return at least one item
            address: Ipv6Addr::from_str(parts.next().ok_or("Empty iterator")?)?,
            prefix: match parts.next() {
                Some(prefix_raw) => u8::from_str(prefix_raw)?,
                None => 64,
            },
        })
    }
}

impl std::fmt::Display for Ipv6Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

#[derive(Debug, Clone)]
pub struct Hostname {
    pub hostname: String,
    pub line: KSLine,
}

impl Hostname {
    pub fn new(hostname: String, line: KSLine) -> Self {
        Self { hostname, line }
    }
}

#[derive(Debug, Clone, Default)]
pub enum DeviceType {
    #[default]
    None,
    Ethernet,
    Bond,
    Bridge,
    Vlan,
}

#[derive(Debug, Clone)]
pub struct Ipv4Netmask(u8);

impl FromStr for Ipv4Netmask {
    type Err = Box<dyn std::error::Error + Send + Sync>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mask = u32::from(Ipv4Addr::from_str(s)?);
        let prefix = mask.leading_ones() as u8;
        if (u64::from(mask) << prefix) & 0xffffffff != 0 {
            Err(format!("Invalid netmask: {}", s))?;
        }

        Ok(Self(prefix))
    }
}

impl std::fmt::Display for Ipv4Netmask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A unique reference to a device + vlanid combination.
///
/// This is used as the key for the netdevs hashmap.
///
/// This is necessary because when a vlanid is defined, we are not configuring
/// the device itself, but rather a subinterface of the device. Hence, we need
/// to be able to differentiate between the interface itself (vlanid=None) and
/// the each subinterface (vlanid=Some(u16)).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct UniqueDeviceReference {
    pub device: DeviceReference,
    pub vlanid: Option<u16>,
}

impl HandleCommand for Network {
    fn handle(mut self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
        let mut result = Ok(());

        // Check if we're configuring the hostname
        if let Some(hostname) = self.hostname.take() {
            if let Some(old) = data.hostname.take() {
                result = Err(SetsailError::new_sem_warn(
                    line.clone(),
                    format!("overriding previous network command at {}", old.line),
                ));
            }

            data.hostname = Some(Hostname::new(hostname, line.clone()));
        }

        // Check if we're configuring a device or just adding generic settings
        if self.device.is_none() {
            return result;
        }

        // Get the device name
        let device_ref = UniqueDeviceReference {
            device: self.device.take().unwrap(),
            vlanid: self.vlanid,
        };

        // Check if we're overriding a previous network command and make a warning about it
        if let Some(old) = data.netdevs.remove(&device_ref) {
            result = Err(SetsailError::new_sem_warn(
                line.clone(),
                format!("overriding previous network command at {}", old.line),
            ));
        }

        // Mark device type
        self.device_type = if !self.bondslaves.is_empty() {
            DeviceType::Bond
        } else if !self.bridgeslaves.is_empty() {
            DeviceType::Bridge
        } else if self.vlanid.is_some() {
            DeviceType::Vlan
        } else {
            DeviceType::Ethernet
        };

        // Check name is valid for device type
        // Bridge and bonds can only have NAMES
        if !matches!(self.device_type, DeviceType::Ethernet)
            && !matches!(device_ref.device, DeviceReference::Name(_))
        {
            return Err(SetsailError::new_semantic(
                line,
                format!(
                    "Only a device NAME can be used for a bond or bridge, not {}",
                    device_ref.device,
                ),
            ));
        }

        // Check if we're overriding the vlan interface name
        // If so, check that it's a valid name
        if let Some(name) = self.interfacename.as_ref() {
            // We can safely unwrap vlanid because it is a requirement for interfacename
            if let Err(err) = validate_vlan_interface_name(name, self.vlanid.unwrap_or(0)) {
                // If we get an error it means the name is not valid
                return Err(SetsailError::new_semantic(line, err));
            }
        }

        // If this is the first device, activate it by default per kickstart spec
        if data.netdevs.is_empty() {
            self.activate = true;
        }

        // If no-activate is set, override
        if self.no_activate {
            self.activate = false;
        }

        // Prepare the device for insertion into the data structure
        self.line = line;
        data.netdevs.insert(device_ref, self);
        result
    }
}

/// Function to check that a vlan interface name follow appropriate conventions.
/// The same convetions are followed in kickstart:
///
/// If the name contains a dot (.), it must take the form of NAME.ID. The NAME is
/// arbitrary, but the ID must be the VLAN ID. For example: em1.171 or my-vlan.171.
/// Names starting with vlan must take the form of vlanID - for example: vlan171.
fn validate_vlan_interface_name(name: &str, vlanid: u16) -> Result<(), String> {
    if name.contains('.') {
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() != 2 {
            Err(format!(
                "VLAN interface name {} must contain exactly one dot",
                name
            ))
        } else if parts[1] != vlanid.to_string() {
            Err(format!(
                "VLAN interface name {} must end with the VLAN ID {}",
                name, vlanid
            ))
        } else {
            Ok(())
        }
    } else if name.starts_with("vlan")
        && name.strip_prefix("vlan") != Some(vlanid.to_string().as_str())
    {
        Err(format!(
            "VLAN interface name {} must contain the VLAN ID {}",
            name, vlanid
        ))
    } else {
        Ok(())
    }
}
