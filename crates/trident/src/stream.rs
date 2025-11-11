use std::{collections::HashMap, time::Duration};

use osutils::lsblk::{self, BlockDeviceType};
use tera::Tera;
use trident_api::{
    config::{self, HostConfiguration},
    error::{InvalidInputError, ReportError, TridentError, TridentResultExt},
};
use url::Url;

use crate::osimage::OsImage;

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

    let template_data = String::from_utf8(template.to_vec())
        .structured(InvalidInputError::LoadCosi {
            url: image_url.clone(),
        })
        .message("Host Configuration template is not valid UTF-8")?;

    let expanded = expand_template(&template_data)
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

fn expand_template(template: &str) -> Result<String, anyhow::Error> {
    struct SizeRange;
    impl tera::Filter for SizeRange {
        fn filter(
            &self,
            value: &tera::Value,
            args: &HashMap<String, tera::Value>,
        ) -> tera::Result<tera::Value> {
            match value.clone() {
                tera::Value::Array(a) => {
                    let low = match args.get("low") {
                        Some(v) => v.as_u64().unwrap_or(0),
                        None => 0,
                    };
                    let high = match args.get("high") {
                        Some(v) => v.as_u64().unwrap_or(u64::MAX),
                        None => u64::MAX,
                    };

                    let filtered: Vec<tera::Value> = a
                        .into_iter()
                        .filter(|item| {
                            if let Some(size) = item.get("size").and_then(|s| s.as_u64()) {
                                size >= low && size <= high
                            } else {
                                false
                            }
                        })
                        .collect();

                    Ok(tera::Value::Array(filtered))
                }
                x => Ok(x),
            }
        }
    }

    let disks = tera::Value::Array(
        lsblk::list()
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.blkdev_type == BlockDeviceType::Disk)
            .filter(|d| {
                d.name.starts_with("sd") || d.name.starts_with("nvme") || d.name.starts_with("vd")
            })
            .map(|b| {
                let mut m = serde_json::Map::new();
                m.insert("name".into(), tera::Value::String(b.name.clone()));
                m.insert(
                    "path".into(),
                    tera::Value::String(format!("/dev/{}", b.name)),
                );
                m.insert("size".into(), tera::Value::Number(b.size.into()));

                println!(
                    "Detected disk: name='{}' path='/dev/{}' size={}",
                    b.name, b.name, b.size
                );
                tera::Value::Object(m)
            })
            .collect(),
    );

    let mut tera = Tera::default();
    tera.register_filter("size_range", SizeRange);
    tera.add_raw_template("config.yaml", template)?;

    let mut context = tera::Context::new();
    context.insert("disks", &disks);
    context.insert("KB", &1024);
    context.insert("MB", &(1024 * 1024));
    context.insert("GB", &(1024 * 1024 * 1024));

    Ok(tera.render("config.yaml", &context)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_template_basic_context() {
        assert_eq!(
            expand_template(
                r#"
kb_value: {{ KB }}
mb_value: {{ MB }}
gb_value: {{ GB }}
"#
            )
            .unwrap(),
            r#"
kb_value: 1024
mb_value: 1048576
gb_value: 1073741824
"#
        );
    }

    #[test]
    fn test_expand_template_detect_disks_function() {
        assert!(expand_template(
            r#"
disks: {{ disks }}
"#
        )
        .is_ok());
    }

    #[test]
    fn test_expand_template_size_range_filter() {
        let result = expand_template(
            r#"
small_disks: {{ disks | size_range(high=1073741824) }}
large_disks: {{ disks | size_range(low=1073741824) }}
"#,
        )
        .unwrap();
        assert!(result.starts_with("\nsmall_disks: "));
        assert!(result.contains("\nlarge_disks: "));
    }

    #[test]
    fn test_expand_template_invalid_syntax() {
        assert!(expand_template(
            r#"
invalid: {{ unclosed_brace
"#
        )
        .is_err());
    }

    #[test]
    fn test_expand_template_combined_features() {
        let result = expand_template(
            r#"
storage:
  min_size: {{ 10 * GB }}
  detected_disks: {{ disks | size_range(low=1073741824) }}
"#,
        )
        .unwrap();
        assert!(result.starts_with("\nstorage:\n  min_size: 10737418240\n  detected_disks: "));
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    #[functional_test]
    fn test_detect_disks() {
        assert_eq!(
            expand_template(
                r#"{{ disks | filter(attribute="name", value="sda") | first | get(key="path")}}"#
            )
            .unwrap(),
            "/dev/sda"
        );
    }
}
