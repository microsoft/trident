use std::{collections::HashMap, time::Duration};

use osutils::lsblk::{self, BlockDeviceType};
use tera::Tera;
use trident_api::{
    config::{self, HostConfiguration},
    error::{InvalidInputError, ReportError, TridentError, TridentResultExt},
};
use url::Url;

use crate::osimage::OsImage;

/// Stream a Host Configuration template from a COSI and expand it.
pub fn config_from_image_url(
    image_url: Url,
    hash: &str,
) -> Result<HostConfiguration, TridentError> {
    let mut image_source = config::OsImage {
        url: image_url.clone(),
        sha384: config::ImageSha384::new(hash)?,
    };

    let image = OsImage::load(&mut image_source, Duration::from_secs(10))
        .message("Failed to download OS image")?;

    let template = image
        .host_configuration_template()
        .structured(InvalidInputError::LoadCosi {
            url: image_url.clone(),
        })
        .message("Image file does not contain a Host Configuration template")?;

    let expanded = expand_template(template)
        .structured(InvalidInputError::LoadCosi {
            url: image_url.clone(),
        })
        .message("Failed to expand Host Configuration template")?;

    let mut config: HostConfiguration = serde_yaml::from_str(&expanded)
        .structured(InvalidInputError::LoadCosi {
            url: image_url.clone(),
        })
        .message("Failed to parse expanded Host Configuration template")?;

    config.image = Some(config::OsImage {
        url: image_url,
        sha384: image_source.sha384,
    });

    Ok(config)
}

/// Use the `tera` templating engine to expand a Host Configuration template.
///
/// See https://keats.github.io/tera/docs for details on the templating syntax.
///
/// # Provided Context
///
/// * `get_disks()`: Returns a list of detected block devices of type `disk`. Accepts optional
///   arguments `min_size` and `max_size` to filter disks by size in bytes. Each disk has the
///   following fields:
///   * `name`: The device name (e.g., `sda`, `nvme0n1`).
///   * `path`: The full device path (e.g., `/dev/sda`, `/dev/nvme0n1`).
///   * `size`: The size of the device in bytes.
///   * `kind`: The kind of device (e.g., `sd`, `nvme`, `vd`, `hd`, `mmcblk`).
///
/// * `KiB`, `MiB`, `GiB`: Constants representing the number of bytes in a kilobyte, megabyte, etc.
///
/// # Examples
///
/// Select the smallest disk at least 10 GiB in size:
/// ```yaml
/// device: "{{ get_disks(min_size=10*GiB) | sort(attribute='size') | first | get(key='path') }}"
/// ```
///
/// Select the largest NVMe disk:
/// ```yaml
/// device: "{{ get_disks() | filter(attribute='kind', value='nvme') | sort(attribute='size') | last | get(key='path') }}"
/// ```
fn expand_template(template: &str) -> Result<String, anyhow::Error> {
    let disks: Vec<_> = lsblk::list()
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.blkdev_type == BlockDeviceType::Disk)
        .filter_map(|b| {
            let kind = ["sd", "nvme", "vd", "hd", "mmcblk"]
                .into_iter()
                .find(|k| b.name.starts_with(*k))?;

            let mut m = serde_json::Map::new();
            m.insert("name".into(), tera::Value::String(b.name.clone()));
            m.insert(
                "path".into(),
                tera::Value::String(format!("/dev/{}", b.name)),
            );
            m.insert("size".into(), tera::Value::Number(b.size.into()));
            m.insert("kind".into(), tera::Value::String(kind.into()));

            Some(tera::Value::Object(m))
        })
        .collect();

    struct GetDisks(Vec<tera::Value>);
    impl tera::Function for GetDisks {
        fn call(&self, args: &HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
            let low = match args.get("min_size") {
                Some(v) => v
                    .as_u64()
                    .ok_or_else(|| tera::Error::msg("Invalid 'min_size' value"))?,
                None => 0,
            };
            let high = match args.get("max_size") {
                Some(v) => v
                    .as_u64()
                    .ok_or_else(|| tera::Error::msg("Invalid 'max_size' value"))?,
                None => u64::MAX,
            };

            let filtered: Vec<tera::Value> = self
                .0
                .iter()
                .filter(|item| {
                    if let Some(size) = item.get("size").and_then(|s| s.as_u64()) {
                        size >= low && size <= high
                    } else {
                        false
                    }
                })
                .cloned()
                .collect();

            Ok(tera::Value::Array(filtered))
        }
    }

    let mut tera = Tera::default();
    tera.register_function("get_disks", GetDisks(disks));
    tera.add_raw_template("config.yaml", template)?;

    let mut context = tera::Context::new();
    context.insert("KiB", &1024);
    context.insert("MiB", &(1024 * 1024));
    context.insert("GiB", &(1024 * 1024 * 1024));

    Ok(tera.render("config.yaml", &context)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    #[test]
    fn test_expand_template_basic_context() {
        assert_eq!(
            expand_template(indoc! {r#"
                kb_value: {{ KiB }}
                mb_value: {{ MiB }}
                gb_value: {{ GiB }}
            "#})
            .unwrap(),
            indoc! {r#"
                kb_value: 1024
                mb_value: 1048576
                gb_value: 1073741824
            "#}
        );
    }

    #[test]
    fn test_expand_template_detect_disks_function() {
        assert!(expand_template(indoc! {r#"
            disks: {{ get_disks() }}
        "#})
        .is_ok());
    }

    #[test]
    fn test_expand_template_size_range_filter() {
        let result = expand_template(indoc! {r#"
            small_disks: {{ get_disks(max_size=1073741824) }}
            large_disks: {{ get_disks(min_size=1073741824 + 1) }}
        "#})
        .unwrap();
        assert!(result.starts_with("small_disks: "));
        assert!(result.contains("\nlarge_disks: "));
    }

    #[test]
    fn test_expand_template_invalid_syntax() {
        assert!(expand_template(indoc! {r#"
            invalid: {{ unclosed_brace
        "#})
        .is_err());
    }

    #[test]
    fn test_expand_template_combined_features() {
        let result = expand_template(indoc! {r#"
            storage:
              min_size: {{ 10 * GiB }}
              detected_disks: {{ get_disks(min_size=1073741824) }}
        "#})
        .unwrap();
        assert!(result.starts_with("storage:\n  min_size: 10737418240\n  detected_disks: "));
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use indoc::indoc;
    use pytest_gen::functional_test;

    #[functional_test]
    fn test_detect_disks() {
        assert_eq!(
            expand_template(indoc! {r#"{{ get_disks() | filter(attribute="name", value="sda") | first | get(key="path")}}"#}).unwrap(),
            "/dev/sda"
        );
    }
}
