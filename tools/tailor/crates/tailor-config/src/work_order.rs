//! Strict, workspace-independent work-order structure. Names are never normalized.

use std::{
    collections::{BTreeMap, BTreeSet},
    slice,
};

use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_yaml_ng::{Mapping, Value};

use crate::ConfigError;

const SCHEMA_VERSION: u64 = 1;
const ROOT_FIELDS: &[&str] = &[
    "schemaVersion",
    "description",
    "images",
    "select",
    "selections",
    "slugs",
    "cells",
];
const SLICE_FIELDS: &[&str] = &["images", "select"];

/// A workspace image slice; each filter applies independently to every named image.
#[derive(Debug, Clone, Serialize)]
pub struct WorkOrderSelection {
    images: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    select: BTreeMap<String, Vec<String>>,
}

impl WorkOrderSelection {
    pub fn images(&self) -> &[String] {
        &self.images
    }

    pub fn select(&self) -> &BTreeMap<String, Vec<String>> {
        &self.select
    }
}

#[derive(Debug, Clone)]
enum Form {
    Images(WorkOrderSelection, Vec<String>),
    Selections(Vec<WorkOrderSelection>, Vec<String>),
    Slugs(Vec<String>),
    Cells(Vec<String>),
}

/// Requested roots, not a frozen build: every form still expands workspace dependencies.
#[derive(Debug, Clone)]
pub struct WorkOrder {
    description: Option<String>,
    form: Form,
}

impl WorkOrder {
    /// Parse one YAML or JSON object. Parser diagnostics deliberately omit raw payloads.
    pub fn parse(input: &str) -> Result<Self, ConfigError> {
        let value: Value = serde_yaml_ng::from_str(input).map_err(|error| {
            let detail = error.location().map_or_else(
                || "invalid YAML/JSON".to_owned(),
                |at| {
                    format!(
                        "invalid YAML/JSON at line {}, column {}",
                        at.line(),
                        at.column()
                    )
                },
            );
            invalid("$", detail)
        })?;
        let root = mapping(&value, "$")?;
        fields(root, ROOT_FIELDS, "$")?;
        if field(root, "schemaVersion").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
            return Err(invalid("schemaVersion", "must be integer 1"));
        }
        let description = field(root, "description")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("description", "must be a string, not null"))
            })
            .transpose()?;
        let images = field(root, "images");
        let selections = field(root, "selections");
        let slugs = field(root, "slugs");
        let cells = field(root, "cells");
        let select = field(root, "select");
        let form = match (images, selections, slugs, cells, select) {
            (Some(_), None, slugs, None, _) => {
                Form::Images(slice(root, "$")?, optional_names(slugs, "slugs")?)
            }
            (None, Some(value), slugs, None, None) => {
                let clauses = value
                    .as_sequence()
                    .filter(|items| !items.is_empty())
                    .ok_or_else(|| invalid("selections", "must be a nonempty list"))?;
                let clauses = clauses
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        let at = format!("selections[{index}]");
                        let map = mapping(value, &at)?;
                        fields(map, SLICE_FIELDS, &at)?;
                        slice(map, &at)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Form::Selections(clauses, optional_names(slugs, "slugs")?)
            }
            (None, None, Some(value), None, None) => Form::Slugs(names(value, "slugs")?),
            (None, None, None, Some(value), None) => Form::Cells(names(value, "cells")?),
            _ => {
                return Err(invalid(
                    "$",
                    "expected exactly one of images, selections, slugs-only or cells-only; select requires images",
                ));
            }
        };
        Ok(Self { description, form })
    }

    /// Construct a resolved order from known, validated, nonempty requested roots.
    pub fn resolved(slugs: Vec<String>) -> Self {
        Self {
            description: None,
            form: Form::Cells(slugs),
        }
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn selections(&self) -> &[WorkOrderSelection] {
        match &self.form {
            Form::Images(selection, _) => slice::from_ref(selection),
            Form::Selections(selections, _) => selections,
            Form::Slugs(_) | Form::Cells(_) => &[],
        }
    }

    /// Exact roots are additive to image slices, never filtered by them.
    pub fn exact_slugs(&self) -> &[String] {
        match &self.form {
            Form::Images(_, slugs)
            | Form::Selections(_, slugs)
            | Form::Slugs(slugs)
            | Form::Cells(slugs) => slugs,
        }
    }
}

impl Serialize for WorkOrder {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schemaVersion", &SCHEMA_VERSION)?;
        if let Some(description) = &self.description {
            map.serialize_entry("description", description)?;
        }
        match &self.form {
            Form::Images(selection, slugs) => {
                map.serialize_entry("images", &selection.images)?;
                if !selection.select.is_empty() {
                    map.serialize_entry("select", &selection.select)?;
                }
                if !slugs.is_empty() {
                    map.serialize_entry("slugs", slugs)?;
                }
            }
            Form::Selections(selections, slugs) => {
                map.serialize_entry("selections", selections)?;
                if !slugs.is_empty() {
                    map.serialize_entry("slugs", slugs)?;
                }
            }
            Form::Slugs(slugs) => map.serialize_entry("slugs", slugs)?,
            Form::Cells(slugs) => map.serialize_entry("cells", slugs)?,
        }
        map.end()
    }
}

fn invalid(field: impl Into<String>, detail: impl Into<String>) -> ConfigError {
    ConfigError::WorkOrder {
        field: field.into(),
        detail: detail.into(),
    }
}

fn field<'a>(map: &'a Mapping, name: &str) -> Option<&'a Value> {
    map.get(Value::String(name.to_owned()))
}

fn mapping<'a>(value: &'a Value, at: &str) -> Result<&'a Mapping, ConfigError> {
    value
        .as_mapping()
        .ok_or_else(|| invalid(at, "must be an object"))
}

fn fields(map: &Mapping, allowed: &[&str], at: &str) -> Result<(), ConfigError> {
    for key in map.keys() {
        let key = key
            .as_str()
            .ok_or_else(|| invalid(at, "field names must be strings"))?;
        if !allowed.contains(&key) {
            return Err(invalid(format!("{at}.{key}"), "unknown field"));
        }
    }
    Ok(())
}

fn names(value: &Value, at: &str) -> Result<Vec<String>, ConfigError> {
    let items = value
        .as_sequence()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| invalid(at, "must be a nonempty list"))?;
    let mut seen = BTreeSet::new();
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let name = item
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| invalid(format!("{at}[{index}]"), "must be a nonblank string"))?;
            if !seen.insert(name) {
                return Err(invalid(format!("{at}[{index}]"), "duplicate value"));
            }
            Ok(name.to_owned())
        })
        .collect()
}

fn optional_names(value: Option<&Value>, at: &str) -> Result<Vec<String>, ConfigError> {
    value
        .map(|value| names(value, at))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn slice(map: &Mapping, at: &str) -> Result<WorkOrderSelection, ConfigError> {
    let images_at = format!("{at}.images");
    let images = names(
        field(map, "images").ok_or_else(|| invalid(&images_at, "required field"))?,
        &images_at,
    )?;
    let mut select = BTreeMap::new();
    if let Some(value) = field(map, "select") {
        let select_at = format!("{at}.select");
        let filters = mapping(value, &select_at)?;
        if filters.is_empty() {
            return Err(invalid(select_at, "must be nonempty"));
        }
        for (axis, values) in filters {
            let axis = axis
                .as_str()
                .filter(|axis| !axis.trim().is_empty())
                .ok_or_else(|| invalid(&select_at, "axis names must be nonblank strings"))?;
            select.insert(
                axis.to_owned(),
                names(values, &format!("{select_at}.{axis}"))?,
            );
        }
    }
    Ok(WorkOrderSelection { images, select })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_forms_and_serialization_round_trip() {
        for input in [
            "schemaVersion: 1\nimages: [gizmo]\nselect: {arch: [amd64]}\nslugs: [other]",
            "schemaVersion: 1\nselections: [{images: [gizmo]}, {images: [qemu]}]\nslugs: [other]",
            r#"{"schemaVersion":1,"slugs":["exact"],"description":""}"#,
            "schemaVersion: 1\ncells: [exact]",
        ] {
            let order = WorkOrder::parse(input).unwrap();
            let serialized = serde_yaml_ng::to_string(&order).unwrap();
            let reparsed = WorkOrder::parse(&serialized).unwrap();
            assert_eq!(serde_yaml_ng::to_string(&reparsed).unwrap(), serialized);
        }
    }

    #[test]
    fn rejects_invalid_structure_without_raw_parser_payload() {
        for extra in [
            "",
            "images: []",
            "images: null",
            "slugs: []",
            "cells: null",
            "slugs: [a, a]",
            "slugs: [' ']",
            "slugs: [1]",
            "select: {arch: [amd64]}",
            "images: [a]\nselect: {}",
            "images: [a]\nselect: null",
            "images: [a]\nselect: {arch: []}",
            "images: [a]\nselect: {arch: [x, x]}",
            "images: [a]\nselect: {' ': [x]}",
            "images: [a]\nslugs: null",
            "images: [a]\ndescription: null",
            "images: [a]\nunknown: secret",
            "images: [a]\ncells: [b]",
            "images: [a]\nselections: [{images: [b]}]",
            "selections: []",
            "selections: null",
            "selections: [{images: []}]",
            "selections: [{images: [a], unknown: true}]",
            "selections: [{images: [a], select: null}]",
            "cells: [a]\nslugs: [b]",
            "cells: [a]\nselect: {arch: [amd64]}",
        ] {
            WorkOrder::parse(&format!("schemaVersion: 1\n{extra}")).unwrap_err();
        }
        for version in ["0", "2", "1.0", "'1'", "null"] {
            WorkOrder::parse(&format!("schemaVersion: {version}\nslugs: [a]")).unwrap_err();
        }
        WorkOrder::parse("slugs: [a]").unwrap_err();
        WorkOrder::parse("schemaVersion: 1\nschemaVersion: 1\nslugs: [a]").unwrap_err();
        let error = WorkOrder::parse("secret malformed [")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
    }
}
