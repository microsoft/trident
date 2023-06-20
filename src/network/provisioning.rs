use std::collections::HashMap;

use super::netplan;

pub fn start(network: Option<serde_yaml::Value>, network_provision: Option<serde_yaml::Value>) {
    let custom_config = network_provision.or(network).and_then(render_yaml);

    match custom_config {
        Some(config) => {
            netplan::write(&config).expect("failed to write netplan config");
            netplan::apply().expect("failed to apply netplan config");
        }
        None => {
            // TODO: implement
            // Today mariner ships with a decent default to do DHCP on all
            // interfaces, and that seems ok for now.
            println!("NETWORK CONFIG NOT PROVIDED!");
        }
    };
}

fn render_yaml(value: serde_yaml::Value) -> Option<String> {
    let final_map = HashMap::from([("network", value)]);

    serde_yaml::to_string(&final_map)
        .map_err(|e| println!("WARN: failed to serialize YAML: {}", e))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::render_yaml;
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

        let parsed = serde_yaml::from_str(sample).expect("Test yaml should be valid!");
        let out = render_yaml(parsed).expect("failed to render test yaml");

        assert_eq!(out, expected);
    }
}
