use std::collections::HashMap;

use lazy_static::lazy_static;
use netplan_types::{NetworkConfig, Renderer};
use regex::Regex;

use crate::config::HostConfigurationStaticValidationError;

const NETPLAN_CONFIG_VERSION: u8 = 2;

lazy_static! {
    /// Regular expression for validating netplan interface names.
    ///
    /// Obtained from:
    /// https://github.com/canonical/netplan/blob/2d3f9044ac63223e7b485b5d0a426c0602b335ce/src/parse.c#L207C32-L207C55
    static ref NETPLAN_ID_REGEX: Regex =
        Regex::new(r"^[[:alnum:][:punct:]]+$").expect("Failed to compile regex");

}

#[cfg(feature = "schemars")]
pub(super) mod schema_helpers {
    use schemars::{gen::SchemaGenerator, schema::Schema};
    use serde_json::{json, Map, Value};

    /// Returns a placeholder schema for a netplan field.
    pub fn make_placeholder_netplan_schema(gen: &mut SchemaGenerator) -> Schema {
        let mut schema = gen
            .subschema_for::<Option<Map<String, Value>>>()
            .into_object();
        schema.format = Some("Netplan YAML".to_owned());
        schema.object().additional_properties = None;
        schema.extensions.insert("nullable".to_owned(), json!(true));
        Schema::Object(schema)
    }
}

fn validate_netplan_id(id: &str) -> Result<(), HostConfigurationStaticValidationError> {
    // Interface names must conform to the netplan ID regex.
    if !NETPLAN_ID_REGEX.is_match(id) {
        return Err(
            HostConfigurationStaticValidationError::InvalidInterfaceName {
                name: id.to_string(),
            },
        );
    }

    // On top of that, they may not contain globbing characters: *?[]
    // https://github.com/canonical/netplan/blob/2d3f9044ac63223e7b485b5d0a426c0602b335ce/src/parse.c#L3183C35-L3183C39
    for invalid_char in "*?[]".chars() {
        if id.contains(invalid_char) {
            return Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: id.to_string(),
                },
            );
        }
    }

    Ok(())
}

pub(super) fn validate_netplan(
    config: &NetworkConfig,
) -> Result<(), HostConfigurationStaticValidationError> {
    if config.version != NETPLAN_CONFIG_VERSION {
        return Err(
            HostConfigurationStaticValidationError::InvalidNetplanVersion {
                version: config.version,
            },
        );
    }

    if let Some(renderer) = &config.renderer {
        if renderer == &Renderer::NetworkManager {
            return Err(
                HostConfigurationStaticValidationError::UnsupportedNetplanRenderer {
                    renderer: "NetworkManager".to_string(),
                },
            );
        }
    }

    fn validate_map_ids<T>(
        map: &Option<HashMap<String, T>>,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        if let Some(map) = map {
            for id in map.keys() {
                validate_netplan_id(id)?;
            }
        }
        Ok(())
    }

    validate_map_ids(&config.ethernets)?;
    validate_map_ids(&config.bridges)?;
    validate_map_ids(&config.bonds)?;
    validate_map_ids(&config.vlans)?;
    validate_map_ids(&config.wifis)?;
    validate_map_ids(&config.tunnels)?;
    validate_map_ids(&config.dummy_devices)?;
    validate_map_ids(&config.vrfs)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_netplan_version() {
        let mut config = NetworkConfig {
            version: 1,
            ..Default::default()
        };
        assert_eq!(
            validate_netplan(&config),
            Err(HostConfigurationStaticValidationError::InvalidNetplanVersion { version: 1 })
        );

        config.version = 3;
        assert_eq!(
            validate_netplan(&config),
            Err(HostConfigurationStaticValidationError::InvalidNetplanVersion { version: 3 })
        );

        config.version = NETPLAN_CONFIG_VERSION;
        assert_eq!(validate_netplan(&config), Ok(()));
    }

    #[test]
    fn test_validate_netplan_renderer() {
        let mut config = NetworkConfig {
            version: NETPLAN_CONFIG_VERSION,
            renderer: Some(Renderer::NetworkManager),
            ..Default::default()
        };
        assert_eq!(
            validate_netplan(&config),
            Err(
                HostConfigurationStaticValidationError::UnsupportedNetplanRenderer {
                    renderer: "NetworkManager".to_string()
                }
            )
        );

        config.renderer = Some(Renderer::Networkd);
        assert_eq!(validate_netplan(&config), Ok(()));
    }

    #[test]
    fn test_validate_netplan_id() {
        validate_netplan_id("eth0").expect("eth0 should be a valid netplan ID");
        validate_netplan_id("eth0.1").expect("eth0.1 should be a valid netplan ID");
        validate_netplan_id("eth0:1").expect("eth0:1 should be a valid netplan ID");
        validate_netplan_id("eth0-1").expect("eth0-1 should be a valid netplan ID");
        validate_netplan_id("eth0+1").expect("eth0+1 should be a valid netplan ID");
        validate_netplan_id("eth0_1").expect("eth0_1 should be a valid netplan ID");
        validate_netplan_id("111111").expect("eth0-1 should be a valid netplan ID");
        validate_netplan_id("l_od/@l-!4(.)%3+#54^23\\n")
            .expect("l_od/@l-!4(.)%3+#54^23\\n should be a valid netplan ID");

        assert_eq!(
            validate_netplan_id("eth0*"),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0*".to_string()
                }
            )
        );

        assert_eq!(
            validate_netplan_id("eth0?"),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0?".to_string()
                }
            )
        );

        assert_eq!(
            validate_netplan_id("eth0["),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0[".to_string()
                }
            )
        );

        assert_eq!(
            validate_netplan_id("eth0]"),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0]".to_string()
                }
            )
        );

        assert_eq!(
            validate_netplan_id("eth 0"),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth 0".to_string()
                }
            )
        );
    }

    #[test]
    fn test_validate_netplan_ids() {
        let mut config = NetworkConfig {
            version: NETPLAN_CONFIG_VERSION,
            ethernets: Some(maplit::hashmap! {
                "eth0".to_string() => Default::default(),
                "eth0.1".to_string() => Default::default(),
                "eth0:1".to_string() => Default::default(),
                "eth0-1".to_string() => Default::default(),
                "eth0+1".to_string() => Default::default(),
                "eth0_1".to_string() => Default::default(),
                "111111".to_string() => Default::default(),
                "l_od/@l-!4(.)%3+#54^23\\n".to_string() => Default::default(),
            }),
            ..Default::default()
        };

        validate_netplan(&config).expect("All IDs should be valid");

        config.ethernets = Some(maplit::hashmap! {
            "eth0*".to_string() => Default::default(),
        });
        assert_eq!(
            validate_netplan(&config),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0*".to_string()
                }
            )
        );

        config.ethernets = Some(maplit::hashmap! {
            "eth0?".to_string() => Default::default(),
        });
        assert_eq!(
            validate_netplan(&config),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0?".to_string()
                }
            )
        );

        config.ethernets = Some(maplit::hashmap! {
            "eth0[".to_string() => Default::default(),
        });
        assert_eq!(
            validate_netplan(&config),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0[".to_string()
                }
            )
        );

        config.ethernets = Some(maplit::hashmap! {
            "eth0]".to_string() => Default::default(),
        });
        assert_eq!(
            validate_netplan(&config),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth0]".to_string()
                }
            )
        );

        config.ethernets = Some(maplit::hashmap! {
            "eth 0".to_string() => Default::default(),
        });
        assert_eq!(
            validate_netplan(&config),
            Err(
                HostConfigurationStaticValidationError::InvalidInterfaceName {
                    name: "eth 0".to_string()
                }
            )
        );
    }
}
