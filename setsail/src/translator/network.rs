use std::{collections::HashMap, str::FromStr};

use netplan_types::{
    AddressMapping, BondConfig, BondParameters, BridgeConfig, BridgeParameters,
    CommonPropertiesAllDevices, CommonPropertiesPhysicalDeviceType, DhcpOverrides, EthernetConfig,
    MatchConfig, NameserverConfig, NetworkConfig, VlanConfig,
};

use trident_api::{config::HostConfiguration, misc::IdGenerator};

use crate::{
    commands::network::{BootProto, DeviceReference, DeviceType, Ipv6Config},
    data::ParsedData,
    SetsailError,
};

/// This enum represents a netplan device
#[derive(Clone, Debug)]
enum NetplanDevice {
    Ethernet(EthernetConfig),
    Bond(BondConfig),
    Bridge(BridgeConfig),
    Vlan {
        vlan: VlanConfig,
        link_name: String,
        link: DeviceReference,
    },
}

/// This is a helper struct that helps us build a netplan config
struct NetplanEnvironment {
    ethernets: HashMap<String, EthernetConfig>,
    bonds: HashMap<String, BondConfig>,
    bridges: HashMap<String, BridgeConfig>,
    vlans: HashMap<String, VlanConfig>,
    vlan_links: Vec<(String, DeviceReference)>,
}

impl NetplanEnvironment {
    fn new() -> Self {
        Self {
            ethernets: HashMap::new(),
            bonds: HashMap::new(),
            bridges: HashMap::new(),
            vlans: HashMap::new(),
            vlan_links: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.ethernets.is_empty() && self.bonds.is_empty() && self.bridges.is_empty()
    }

    fn add_device(&mut self, name: String, dev: NetplanDevice) {
        match dev {
            NetplanDevice::Ethernet(eth) => {
                self.ethernets.insert(name, eth);
            }
            NetplanDevice::Bond(bond) => {
                self.bonds.insert(name, bond);
            }
            NetplanDevice::Bridge(bridge) => {
                self.bridges.insert(name, bridge);
            }
            NetplanDevice::Vlan {
                vlan,
                link_name,
                link,
            } => {
                self.vlans.insert(name, vlan);
                self.vlan_links.push((link_name, link));
            }
        }
    }

    fn has_device(&self, name: &str) -> bool {
        self.ethernets.contains_key(name)
            || self.bonds.contains_key(name)
            || self.bridges.contains_key(name)
    }

    /// Make sure all vlans' link points to an existing netplan deviceID
    fn finalize_vlans(&mut self) {
        for (name, link) in self.vlan_links.iter() {
            if self.has_device(name) {
                // The device exists, no need to do anything
                continue;
            }

            // Device was not defined, we need to create it
            // We assume it's an ethernet device
            let mut eth = EthernetConfig::default();

            // We only need to edit eth device if the link is a DeviceReference::Mac
            if let DeviceReference::Mac(mac) = link {
                eth.common_physical = Some(CommonPropertiesPhysicalDeviceType {
                    r#match: Some(MatchConfig {
                        macaddress: Some(mac.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }

            self.ethernets.insert(name.clone(), eth);
        }
    }

    fn finalize_bonds(&mut self) {
        for bond in self.bonds.values() {
            if let Some(interfaces) = bond.interfaces.as_ref() {
                for name in interfaces.iter() {
                    if self.has_device(name) {
                        // The device exists, no need to do anything
                        continue;
                    }

                    // Device was not defined, we need to create it
                    // We assume it's an ethernet device
                    self.ethernets
                        .insert(name.clone(), EthernetConfig::default());
                }
            }
        }
    }

    fn finalize_bridges(&mut self) {
        for bridge in self.bridges.values() {
            if let Some(interfaces) = bridge.interfaces.as_ref() {
                for name in interfaces.iter() {
                    if self.has_device(name) {
                        // The device exists, no need to do anything
                        continue;
                    }

                    // Device was not defined, we need to create it
                    // We assume it's an ethernet device
                    self.ethernets
                        .insert(name.clone(), EthernetConfig::default());
                }
            }
        }
    }

    fn get_netplan(mut self) -> NetworkConfig {
        // Make sure all vlans' link points to an existing netplan deviceID
        self.finalize_vlans();

        // Make sure all bond and bridge members are existing netplan deviceIDs
        self.finalize_bonds();
        self.finalize_bridges();

        let mut netplan = NetworkConfig {
            version: 2,
            ..Default::default()
        };

        if !self.ethernets.is_empty() {
            netplan.ethernets = Some(self.ethernets)
        }

        if !self.bonds.is_empty() {
            netplan.bonds = Some(self.bonds)
        }

        if !self.bridges.is_empty() {
            netplan.bridges = Some(self.bridges)
        }

        if !self.vlans.is_empty() {
            netplan.vlans = Some(self.vlans)
        }

        netplan
    }
}

/// This is a helper struct that helps us manage the active and inactive interfaces
/// Active == should be present in provisioning environment and runtime
/// Inactive == should be present in runtime but not provisioning environment
struct EnvironmentManager {
    active: NetplanEnvironment,
    inactive: NetplanEnvironment,
}

impl EnvironmentManager {
    fn new() -> Self {
        Self {
            active: NetplanEnvironment::new(),
            inactive: NetplanEnvironment::new(),
        }
    }

    fn add_device<T>(&mut self, name: String, activate: bool, dev: NetplanDevice)
    where
        NetplanDevice: From<T>,
    {
        if activate {
            self.active.add_device(name.clone(), dev.clone())
        }

        self.inactive.add_device(name, dev);
    }

    fn populate(self, hc: &mut HostConfiguration) {
        // Always populate netplan so only active devices are present in the
        // provisioning environment
        hc.management_os.netplan = Some(self.active.get_netplan());

        if !self.inactive.is_empty() {
            hc.os.netplan = Some(self.inactive.get_netplan());
        }
    }
}

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration, errors: &mut Vec<SetsailError>) {
    // Manager to handle active and inactive interfaces
    let mut envmgr = EnvironmentManager::new();

    // Interface ID generator for nameless devices
    // mostly used for match:mac devices
    let mut idgen = IdGenerator::new("netdev");

    for (k, net) in input.netdevs.iter() {
        let mut netplan_name = match &k.device {
            // Only device references of type "Name" are literal names that
            // can be used in netplan
            DeviceReference::Name(name) => name.clone(),
            DeviceReference::Mac(_) /*| DeviceReference::Link*/ => idgen.next_id(),
        };

        // Init common properties for devices
        let mut common_all = CommonPropertiesAllDevices {
            // Clear link local
            link_local: Some(vec![]),
            ..Default::default()
        };

        // Set common properties

        // Set DHCP to the value of bootproto
        if !net.noipv4 {
            match net.bootproto {
                BootProto::Dhcp => {
                    common_all.dhcp4 = Some(true);
                }
                BootProto::Static => {
                    common_all.dhcp4 = Some(false);
                    common_all.addresses = Some(vec![AddressMapping::Simple(format!(
                        "{}/{}",
                        net.ip
                            .as_ref()
                            .expect("This should be checked in the parser"),
                        net.netmask
                            .as_ref()
                            .expect("This should be checked in the parser")
                    ))]);
                }
            }
        }

        // Set DHCPv6
        if !net.noipv6 {
            match net.ipv6 {
                Ipv6Config::Auto => {
                    // Just set up link local
                    // Setting to None seems counterintuitive, but this way we use netplan's default
                    // which is to use enable link-local for IPv6 only.
                    common_all.link_local = None;
                }
                Ipv6Config::Dhcp => {
                    common_all.dhcp6 = Some(true);
                }
                Ipv6Config::Static(ipv6) => {
                    common_all.addresses = Some(vec![AddressMapping::Simple(ipv6.to_string())]);
                }
            }
        }

        // Set MTU
        if let Some(mtu) = net.mtu {
            common_all.mtu = Some(mtu);
        }

        // Set gateway IPv4
        if let Some(gateway) = net.gateway {
            common_all.gateway4 = Some(gateway.to_string());
        }

        // Set gateway IPv6
        if let Some(gateway) = net.ipv6gateway {
            common_all.gateway6 = Some(gateway.to_string());
        }

        // Set nameservers
        if net.nodns {
            // When nodns is set, we don't set up any DNS ourselves, and we disable
            // DNS in DHCP
            let overrides = DhcpOverrides {
                use_dns: Some(false),
                ..Default::default()
            };

            common_all.dhcp4_overrides = Some(overrides.clone());
            common_all.dhcp6_overrides = Some(overrides);
        } else {
            // Mutable object we will add all config to
            let mut nameserver_config = NameserverConfig::default();

            // Netplan doesn't differentiate between IPv4 and IPv6 search domains, so we just merge them
            let mut dns_search: Vec<String> = Vec::new();
            dns_search.extend(
                net.ipv4_dns_search
                    .iter()
                    .map(|ns| ns.to_string())
                    .collect::<Vec<String>>(),
            );
            dns_search.extend(
                net.ipv6_dns_search
                    .iter()
                    .map(|ns| ns.to_string())
                    .collect::<Vec<String>>(),
            );

            // If we have any dns search domains, we add them to the nameserver config
            if !dns_search.is_empty() {
                nameserver_config.search = Some(dns_search);
            }

            // If we have any nameservers, we add them to the nameserver config
            if !net.nameserver.is_empty() {
                nameserver_config.addresses =
                    Some(net.nameserver.iter().map(|ns| ns.to_string()).collect());
            }

            // Check if we made any modifications to the nameserver config object
            // If we did, we add it to the common_all object
            if nameserver_config != NameserverConfig::default() {
                common_all.nameservers = Some(nameserver_config);
            }

            // Check if disabling DHCPv4 DNS was requested
            if net.ipv4_ignore_auto_dns {
                // If we do, we set the DHCP overrides
                common_all.dhcp4_overrides = Some(DhcpOverrides {
                    use_dns: Some(false),
                    ..Default::default()
                });
            }

            // Check if disabling DHCPv6 DNS was requested
            if net.ipv6_ignore_auto_dns {
                // If we do, we set the DHCP overrides
                common_all.dhcp6_overrides = Some(DhcpOverrides {
                    use_dns: Some(false),
                    ..Default::default()
                });
            }
        }

        // TODO:
        // Set hostname
        // In kickstart:
        //      The host name can either be a fully-qualified domain name (FQDN) in
        //      the format hostname.domainname, or a short host name with no domain.
        //      Many networks have a DHCP service which automatically supplies connected
        //      systems with a domain name; to allow DHCP to assign the domain name,
        //      only specify a short host name.
        // We are not making that distinction yet.
        // It should be done by setting the dhcp-override `use-hostname: false`

        let device: NetplanDevice = match net.device_type {
            DeviceType::None => {
                // Note: we should _never_ get here
                errors.push(SetsailError::new_translation(
                    net.line.clone(),
                    "the parser should never return a device with no type".into(),
                ));
                continue;
            }
            DeviceType::Vlan => {
                // For vlans, we redefine the netplan_name to be LINK.ID (or a user-provided name)
                // We save the original netplan_name in link_name to use as the name of the base interface
                let link_name = netplan_name.clone();
                netplan_name = match net.interfacename.as_ref() {
                    // The validity of this name is checked in the parser
                    Some(name) => name.to_string(),
                    // If no name is provided, we use LINK.ID
                    None => format!(
                        "{}.{}",
                        netplan_name,
                        net.vlanid.expect("This should be checked in the parser")
                    ),
                };

                // Note: vlan.link needs to be an existing netplan device ID. We need to wait until all devices
                // are translated to check if the link device exists, or if it needs to be created.
                // This is done in the EnvironmentManager::finalize_vlans function
                let vlan = VlanConfig {
                    // This should always be Some, as it's checked in the parser
                    id: net.vlanid,
                    common_all: Some(common_all),
                    link: Some(link_name.clone()),
                };

                NetplanDevice::Vlan {
                    vlan,
                    link_name,
                    link: k.device.clone(),
                }
            }
            DeviceType::Ethernet => {
                let mut common_phys = CommonPropertiesPhysicalDeviceType::default();
                if let DeviceReference::Mac(mac) = k.device {
                    common_phys.r#match = Some(MatchConfig {
                        macaddress: Some(mac.to_string()),
                        ..Default::default()
                    });
                };

                NetplanDevice::Ethernet(EthernetConfig {
                    common_all: Some(common_all),
                    common_physical: some_if_not_default(common_phys),
                    ..Default::default()
                })
            }
            DeviceType::Bond => NetplanDevice::Bond(BondConfig {
                common_all: Some(common_all),
                interfaces: Some(net.bondslaves.clone()),
                parameters: match net.bondopts.map(map_bond_opts) {
                    Ok(params) => Some(params),
                    Err(errs) => {
                        for err in errs {
                            errors.push(SetsailError::new_translation(
                                net.line.clone(),
                                format!("failed to parse bond parameters: {err}"),
                            ));
                        }
                        continue;
                    }
                },
            }),
            DeviceType::Bridge => NetplanDevice::Bridge(BridgeConfig {
                common_all: Some(common_all),
                interfaces: Some(net.bridgeslaves.clone()),
                parameters: match net.bridgeopts.map(map_bridge_opts) {
                    Ok(params) => Some(params),
                    Err(errs) => {
                        for err in errs {
                            errors.push(SetsailError::new_translation(
                                net.line.clone(),
                                format!("failed to parse bridge parameters: {err}"),
                            ));
                        }
                        continue;
                    }
                },
            }),
        };

        envmgr.add_device(netplan_name, net.activate, device);
    }

    envmgr.populate(hc);
}

fn some_if_not_default<T: Default + PartialEq>(value: T) -> Option<T> {
    if value == T::default() {
        None
    } else {
        Some(value)
    }
}

/// This function translates the kickstart names to netplan names
/// Kickstart and Netplan use different names for some bond configuration options
/// Kickstart uses the names in the kernel documentation
/// <https://www.kernel.org/doc/Documentation/networking/bonding.txt>
fn map_bond_opts(key: &str, value: &str, opts: &mut BondParameters) -> Result<(), String> {
    // Imports only used here
    use netplan_types::{
        AdSelect, ArpAllTargets, ArpValidate, BondMode, FailOverMacPolicy, LacpRate,
        PrimaryReselectPolicy, TransmitHashPolicy,
    };
    use std::net::IpAddr;

    // We'll be using this a lot
    let invalid = Err("Invalid value".into());
    match key {
        "ad_select" => {
            opts.ad_select = Some(match value {
                "0" | "stable" => AdSelect::Stable,
                "1" | "bandwidth" => AdSelect::Bandwidth,
                "2" | "count" => AdSelect::Count,
                _ => return invalid,
            });
        }
        "all_slaves_active" => {
            opts.all_slaves_active = Some(match value {
                "0" | "dropped" => false,
                "1" | "delivered" => true,
                _ => return invalid,
            });
        }
        "arp_all_targets" => {
            opts.arp_all_targets = Some(match value {
                "0" | "any" => ArpAllTargets::Any,
                "1" | "all" => ArpAllTargets::All,
                _ => return invalid,
            });
        }
        "arp_interval" => {
            opts.arp_interval = Some(ensure_parse::<u32>(value)?);
        }
        "arp_ip_target" => {
            // Check that all IPs are valid, otherwise return an error
            opts.arp_ip_targets = match value
                .split(',')
                .try_for_each(|v| IpAddr::from_str(v).and(Ok(())))
            {
                Ok(_) => Some(value.split(',').map(|s| s.into()).collect()),
                Err(e) => return Err(e.to_string()),
            }
        }
        "arp_validate" => {
            opts.arp_validate = Some(match value {
                "0" | "none" => ArpValidate::None,
                "1" | "active" => ArpValidate::Active,
                "2" | "backup" => ArpValidate::Backup,
                "3" | "all" => ArpValidate::All,
                _ => return invalid,
            });
        }
        "downdelay" => {
            opts.down_delay = Some(ensure_parse::<u32>(value)?);
        }
        "fail_over_mac" => {
            opts.fail_over_mac_policy = Some(match value {
                "0" | "none" => FailOverMacPolicy::None,
                "1" | "active" => FailOverMacPolicy::Active,
                "2" | "follow" => FailOverMacPolicy::Follow,
                _ => return invalid,
            });
        }
        "lacp_rate" => {
            opts.lacp_rate = Some(match value {
                "0" | "slow" => LacpRate::Slow,
                "1" | "fast" => LacpRate::Fast,
                _ => return invalid,
            });
        }
        "lp_interval" => {
            opts.learn_packet_interval = Some(ensure_parse::<u32>(value)?);
        }
        "miimon" => {
            opts.mii_monitor_interval = Some(ensure_parse::<u32>(value)?);
        }
        "min_links" => {
            opts.min_links = Some(value.parse::<u16>().map_err(|e| e.to_string())?);
        }
        "mode" => {
            opts.mode = Some(match value {
                "0" | "balance-rr" => BondMode::BalanceRr,
                "1" | "active-backup" => BondMode::ActiveBackup,
                "2" | "balance-xor" => BondMode::BalanceXor,
                "3" | "broadcast" => BondMode::Broadcast,
                "4" | "802.3ad" => BondMode::EightZeroTwoDotThreeAD,
                "5" | "balance-tlb" => BondMode::BalanceTlb,
                "6" | "balance-alb" => BondMode::BalanceAlb,
                _ => return invalid,
            })
        }
        "num_grat_arp" => {
            opts.gratuitous_arp = Some(value.parse::<u8>().map_err(|e| e.to_string())?);
        }
        "packets_per_slave" => {
            opts.packets_per_slave = Some(value.parse::<u32>().map_err(|e| e.to_string())?);
        }
        "primary" => {
            opts.primary = Some(value.to_string());
        }
        "primary_reselect" => {
            opts.primary_reselect_policy = Some(match value {
                "0" | "always" => PrimaryReselectPolicy::Always,
                "1" | "better" => PrimaryReselectPolicy::Better,
                "2" | "failure" => PrimaryReselectPolicy::Failure,
                _ => return invalid,
            });
        }
        "resend_igmp" => {
            opts.resend_igmp = Some(value.parse::<u8>().map_err(|e| e.to_string())?);
        }
        "updelay" => {
            opts.up_delay = Some(ensure_parse::<u32>(value)?);
        }
        "xmit_hash_policy" => {
            opts.transmit_hash_policy = Some(match value {
                "layer2" => TransmitHashPolicy::Layer2,
                // TODO: layer2+3 is not supported by netplan_types, but netplan now supports it
                "layer3+4" => TransmitHashPolicy::Layer3Plus4,
                "encap2+3" => TransmitHashPolicy::Encap2Plus3,
                "encap3+4" => TransmitHashPolicy::Encap3Plus4,
                _ => return invalid,
            });
        }
        _ => return Err("Unsupported option".into()),
    }

    Ok(())
}

/// This is a shorthand to make sure a value can be properly parsed
/// as a specific type, but we still want the original string anyway
fn ensure_parse<T>(value: &str) -> Result<String, String>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    match value.parse::<T>() {
        Ok(_) => Ok(value.to_string()),
        Err(e) => Err(format!("failed to parse value: {e}")),
    }
}

/// This function translates bond options into netplan
fn map_bridge_opts(key: &str, value: &str, opts: &mut BridgeParameters) -> Result<(), String> {
    println!("{key} = {value}");
    match key {
        "stp" => {
            opts.stp = Some(match value {
                "0" | "t" | "true" | "y" | "yes" | "off" => false,
                "1" | "f" | "false" | "n" | "no" | "on" => true,
                _ => return Err("Invalid value".into()),
            });
        }
        "priority" => {
            opts.priority = Some(value.parse::<u32>().map_err(|e| e.to_string())?);
        }
        "forward-delay" => opts.forward_delay = Some(ensure_parse::<u32>(value)?),
        "hello-time" => opts.hello_time = Some(ensure_parse::<u32>(value)?),
        "max-age" => opts.max_age = Some(ensure_parse::<u32>(value)?),
        "ageing-time" => opts.ageing_time = Some(ensure_parse::<u32>(value)?),
        _ => return Err("Unsupported option".into()),
    }

    Ok(())
}
