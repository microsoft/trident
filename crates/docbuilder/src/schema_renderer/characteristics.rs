use anyhow::{Context, Error};
use serde::Serialize;

/// Struct to represent a specific characteristic of a node.
#[derive(Debug, Clone, Serialize, Default)]
struct Characteristic {
    name: String,
    value: String,
    is_markdown: bool,
}

impl Characteristic {
    fn new(name: impl AsRef<str>, value: impl Into<String>, is_markdown: bool) -> Self {
        Self {
            name: name.as_ref().to_owned(),
            value: value.into(),
            is_markdown,
        }
    }

    fn new_markdown(name: impl AsRef<str>, value: impl Into<String>) -> Self {
        Self::new(name, value, true)
    }

    fn new_value(name: impl AsRef<str>, value: impl Into<String>) -> Self {
        Self::new(name, value, false)
    }
}

/// Struct to hold characteristics of a node.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(transparent)]
pub(super) struct Characteristics {
    characteristics: Vec<Characteristic>,
}

impl Characteristics {
    /// Add a characteristic where the value is raw markdown.
    ///
    /// The value will be rendered to markdown as-is.
    pub(super) fn push_markdown(&mut self, name: impl AsRef<str>, value: impl Into<String>) {
        self.characteristics
            .push(Characteristic::new_markdown(name, value));
    }

    /// Add a characteristic where the value is a raw value.
    ///
    /// The value will be rendered to markdown as a code block.
    pub(super) fn push(&mut self, key: impl AsRef<str>, value: impl Into<String>) {
        self.characteristics
            .push(Characteristic::new_value(key, value));
    }

    /// Add a characteristic where the value is a raw value.
    ///
    /// The value will be rendered to markdown as a code block.
    pub(super) fn push_value(
        &mut self,
        key: impl AsRef<str>,
        value: &serde_json::Value,
    ) -> Result<(), Error> {
        self.characteristics.push(Characteristic::new_value(
            key.as_ref(),
            serde_json::to_string(value).with_context(|| {
                format!(
                    "Failed to serialize value for characteristic '{}'",
                    key.as_ref()
                )
            })?,
        ));
        Ok(())
    }

    /// Add a characteristic where the value is a raw value.
    ///
    /// The value will be rendered to markdown as a code block.
    pub(super) fn push_display(&mut self, key: impl AsRef<str>, value: impl std::fmt::Display) {
        self.characteristics
            .push(Characteristic::new_value(key, value.to_string()));
    }
}
