use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct FrontMatter {
    fields: BTreeMapMap<String, String>,
}

impl FrontMatter {
    pub(super) fn with_field(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.fields
            .insert(key.as_ref().to_string(), value.as_ref().to_string());
        self
    }

    pub(super) fn render(&self) -> String {
        let mut fm = String::from("---\n");
        for (key, value) in &self.fields {
            fm.push_str(&format!("{}: {}\n", key, value));
        }
        fm.push_str("---\n\n");
        fm
    }
}
