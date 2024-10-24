use std::collections::HashSet;

use anyhow::{bail, ensure, Context, Error};

use serde_json::Value;
use trident_api::{
    primitives::shortcuts::STRING_SHORTCUT_EXTENSION,
    schemars::schema::{InstanceType, ObjectValidation, Schema, SchemaObject, SingleOrVec},
};

use super::characteristics::Characteristics;

/// A model of a Node in the schema
///
/// This is a model of a Node in the schema. It is used to generate
/// documentation. It simplifies the structure of JSON Schema into a more easily
/// understandable format that can be easily used to populate a template.
///
/// It does not support all JSON schema features, just ones that we know are
/// created by schemars.
///
/// Current limitations:
///
/// - `type` as described in the JSON schema docs can be a string with the name
///   of a type, or an array of strings with names of types if several are
///   allowed. Here only single types are supported.

#[derive(Debug, Clone)]
pub(crate) struct SchemaNodeModel {
    // Metadata fields:
    /// Name of the Node.
    pub(crate) name: Option<String>,

    /// Description of the Node.
    pub(crate) description: Option<String>,

    /// Default value of the Node.
    pub(crate) default: Option<Value>,

    /// Example values of the Node.
    #[allow(dead_code)] // Will be used in the future!
    pub(crate) examples: Vec<Value>,

    /// Whether the Node is deprecated.
    pub(crate) deprecated: bool,

    /// Whether the Node is read-only.
    pub(crate) read_only: bool,

    /// Whether the Node is write-only.
    pub(crate) write_only: bool,

    // Object fields:
    /// Format of the Node.
    pub(crate) format: Option<String>,

    /// Kind of node.
    pub(crate) kind: NodeKind,

    /// The object that this node is based on.
    pub(crate) object: SchemaObject,
}

/// The kind of node that can be created by schemars.
///
/// Does not necessarily correspond to the kind of node that can exist in JSON
/// Schema, but rather what we consider it to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NodeKind {
    // * * * * * * * * *
    // * Complex types *
    // * * * * * * * * *
    /// The node is a full object.
    Object,

    /// The node is referencing a definition.
    ///
    /// schemars uses `allOf` with a single reference to a definition to represent
    /// this case.
    DefinitionReference,

    /// The node is an enum.
    ///
    /// schemars uses `oneOf` with a list of enum values to represent this case.
    Enum,

    /// The node is an object reference with a string shortcut.
    ///
    /// This node is really the referenced type, but the deserializer can populate it from just a string.
    RefWithStringShortcut,

    // * * * * * * * * *
    // * Simple types  *
    // * * * * * * * * *
    /// The node is a simple map.
    ///
    /// A map will necessarily have a single type of object as its value.
    Map(InstanceType),

    /// The node is a simple object.
    ///
    /// A simple object is a node of type object that contains no object validation.
    /// It can be rendered as a simple type.
    SimpleObject,

    /// The node is a simple enum.
    ///
    /// schemars uses `enum` with a list of enum values to represent this case.
    ///
    /// This is different from `Enum` because it only contains a simple type.
    SimpleEnum(InstanceType),

    /// Wrapper enum
    ///
    /// This is a simple enum with just one variant. Generally, these exist as wrappers
    /// created by schemars to represent a variant of a complex enum.
    WrapperEnum(InstanceType),

    /// Node is a compound scalar.
    CompoundScalar(Vec<InstanceType>),

    /// The node is a number.
    Number,

    /// The node is an integer.
    Integer,

    /// The node is a string.
    String,

    /// The node is an array.
    Array,

    /// The node is a boolean.
    Boolean,

    /// The node is a reference to a definition.
    Reference,

    /// The node is null.
    Null,
}

impl NodeKind {
    /// Friendly user-facing name of the node kind.
    pub(super) fn name(&self) -> String {
        match self {
            Self::Object => "object".into(),
            Self::DefinitionReference => "reference".into(),
            Self::Enum => "enum".into(),
            Self::Map(_) => "map".into(),
            Self::SimpleObject => "object".into(),
            Self::SimpleEnum(_) => "enum".into(),
            Self::Number => "number".into(),
            Self::Integer => "integer".into(),
            Self::String => "string".into(),
            Self::Array => "array".into(),
            Self::Boolean => "boolean".into(),
            Self::Reference => "reference".into(),
            Self::Null => "null".into(),
            Self::CompoundScalar(l) => l
                .iter()
                .map(|it| instance_type_name(*it))
                .collect::<Vec<_>>()
                .join("/"),
            Self::WrapperEnum(s) => instance_type_name(*s).into(),
            Self::RefWithStringShortcut => "string/map".into(),
        }
    }
}

fn is_scalar(it: InstanceType) -> bool {
    match it {
        InstanceType::Null
        | InstanceType::Boolean
        | InstanceType::Number
        | InstanceType::String
        | InstanceType::Integer => true,
        InstanceType::Array | InstanceType::Object => false,
    }
}

fn get_schema_instance_type(schema: &Schema) -> Result<Option<InstanceType>, Error> {
    match schema {
        Schema::Bool(_) => bail!("Boolean schema has no instance type"),
        Schema::Object(obj) => get_schema_object_instance_type(obj),
    }
}

fn get_schema_object_instance_type(
    schema_obj: &SchemaObject,
) -> Result<Option<InstanceType>, Error> {
    schema_obj.instance_type.as_ref().map_or_else(
        || Ok(None),
        |single_or_vec| match single_or_vec {
            SingleOrVec::Single(instance_type) => Ok(Some(**instance_type)),
            SingleOrVec::Vec(_) => bail!("Multiple instance types not supported"),
        },
    )
}

impl TryFrom<&SchemaObject> for SchemaNodeModel {
    type Error = Error;

    fn try_from(schema: &SchemaObject) -> Result<Self, Error> {
        Self::try_from(schema.clone())
    }
}

impl TryFrom<SchemaObject> for SchemaNodeModel {
    type Error = Error;

    fn try_from(mut schema: SchemaObject) -> Result<Self, Error> {
        // Get the reported instance type.
        let instance_type = schema.instance_type.as_ref();

        // Try to deduce the type of node.
        let kind = if let Some(ref enum_values) = schema.enum_values {
            ensure!(!enum_values.is_empty(), "Enum has no values");
            match instance_type {
                Some(SingleOrVec::Single(single_instance_type)) => {
                    // If the schema has a simple enum defined, then it's an simple enum.
                    if enum_values.len() == 1 {
                        NodeKind::WrapperEnum(**single_instance_type)
                    } else {
                        NodeKind::SimpleEnum(**single_instance_type)
                    }
                }
                Some(SingleOrVec::Vec(_)) => bail!("Enum has multiple instance types"),
                None => bail!("Enum has no valid instance type"),
            }
        } else if schema.is_ref() {
            NodeKind::Reference
        } else {
            // Otherwise we need to figure out the kind of node.
            match instance_type {
                // If the instance type is Some(Vec(_)), then we have to check thatg all values are scalar.
                Some(SingleOrVec::Vec(vec)) => {
                    if vec.iter().all(|it| is_scalar(*it)) {
                        // If all values are scalar, then it's a compound scalar.
                        NodeKind::CompoundScalar(vec.clone())
                    } else {
                        // Otherwise it's a complex type which we don't support yet.
                        bail!("Unsupported complex instance type:\n{:#?}", vec);
                    }
                }

                // If the instance type is Some(Single(_)), then we can easily translate.
                Some(SingleOrVec::Single(single_instance_type)) => match **single_instance_type {
                    InstanceType::Null => NodeKind::Null,
                    InstanceType::Boolean => NodeKind::Boolean,
                    InstanceType::Array => NodeKind::Array,
                    InstanceType::Number => NodeKind::Number,
                    InstanceType::String => NodeKind::String,
                    InstanceType::Integer => NodeKind::Integer,
                    InstanceType::Object => {
                        if let Some(obj_validation) = schema.object.as_ref() {
                            if **obj_validation == ObjectValidation::default() {
                                // If the object validation is the default, then it's a simple object.
                                NodeKind::SimpleObject
                            } else if let Some(additional_properties) =
                                &obj_validation.additional_properties
                            {
                                // If the object validation has additional properties it may be a map.
                                if matches!(**additional_properties, Schema::Object(_)) {
                                    // If the object validation has additional
                                    // properties, AND additional_properties is
                                    // a schema object, then it's a map.
                                    NodeKind::Map(
                                        get_schema_instance_type(additional_properties)?
                                            .context("Map instance type has no instance type")?,
                                    )
                                } else {
                                    // Otherwise it's a full object.
                                    NodeKind::Object
                                }
                            } else {
                                // Otherwise it's a full object.
                                NodeKind::Object
                            }
                        } else {
                            bail!("Object instance type has no object validation");
                        }
                    }
                },
                // If none, it's more complicated. We need to figure out the kind of subschema.
                None => {
                    match schema.subschemas.as_ref() {
                        Some(subschemas) => {
                            if subschemas.all_of.as_ref().is_some_and(|l| l.len() == 1) {
                                // Schemars uses `allOf` with a single reference object to
                                // represent a reference to a definition.
                                NodeKind::DefinitionReference
                            } else if subschemas.one_of.as_ref().is_some_and(|l| !l.is_empty()) {
                                // Check for a custom extension that indicates a map with a string shortcut.
                                if schema
                                    .extensions
                                    .get(STRING_SHORTCUT_EXTENSION)
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or_default()
                                {
                                    let one_of = subschemas.one_of.as_ref().unwrap();
                                    ensure!(
                                        (2..=3).contains(&one_of.len()),
                                        "Expected 2 or 3 oneOf subschemas"
                                    );

                                    ensure!(
                                        one_of.iter().filter(|s| s.is_ref()).count() == 1,
                                        "Expected one reference"
                                    );

                                    let mut options = one_of
                                        .iter()
                                        .map(|s| {
                                            get_schema_instance_type(s)
                                                .context("Failed to get instance type")
                                        })
                                        .collect::<Result<Vec<_>, _>>()
                                        .context("Failed to get instance types")?
                                        .into_iter()
                                        .flatten()
                                        .collect::<HashSet<_>>();

                                    // Drain expected options with instance types.
                                    ensure!(
                                        options.remove(&InstanceType::String),
                                        "String not found"
                                    );

                                    // We may have a third null option.
                                    if !options.is_empty() {
                                        ensure!(
                                            options.remove(&InstanceType::Null),
                                            "Null not found"
                                        );
                                    }

                                    NodeKind::RefWithStringShortcut
                                } else {
                                    // Schemars uses `oneOf` with a list of objects to represent an enum.
                                    NodeKind::Enum
                                }
                            } else {
                                // If we don't know what it is, we can't render it.
                                bail!("Unsupported subschema type:\n{:#?}", subschemas);
                            }
                        }
                        None => {
                            // If we don't know what it is, we can't render it.
                            bail!("Unsupported schema type:\n{:#?}", schema);
                        }
                    }
                }
            }
        };

        Ok(Self {
            name: schema.metadata().title.clone(),
            description: schema.metadata().description.clone(),
            default: schema.metadata().default.clone(),
            examples: schema.metadata().examples.clone(),
            deprecated: schema.metadata().deprecated,
            read_only: schema.metadata().read_only,
            write_only: schema.metadata().write_only,
            format: schema.format.clone(),
            kind,
            object: schema,
        })
    }
}

impl SchemaNodeModel {
    pub(super) fn type_name(&self) -> Result<String, Error> {
        Ok(match &self.kind {
            NodeKind::DefinitionReference
            | NodeKind::Reference
            | NodeKind::RefWithStringShortcut => {
                self.get_reference().context("Failed to get reference")?
            }
            s => s.name(),
        })
    }

    pub(super) fn get_reference(&self) -> Result<String, Error> {
        Ok(match self.kind {
            NodeKind::DefinitionReference => {
                let ref_schema: &Schema = self
                    .object
                    .subschemas
                    .as_ref()
                    .and_then(|t| t.all_of.iter().flatten().find(|s| s.is_ref()))
                    .context("Node is not a definition reference")?;

                get_reference_name(ref_schema)
                    .context("Failed to get reference name")?
                    .to_owned()
            }
            NodeKind::Reference => self
                .object
                .reference
                .as_ref()
                .context("Reference node has no reference")?
                .as_str()
                .trim_start_matches("#/definitions/")
                .to_owned(),
            NodeKind::RefWithStringShortcut => {
                let ref_schema = self
                    .object
                    .subschemas
                    .as_ref()
                    .and_then(|t| t.one_of.iter().flatten().find(|s| s.is_ref()))
                    .context("Node is not a reference with string shortcut")?;

                get_reference_name(ref_schema)
                    .context("Failed to get reference name")?
                    .to_owned()
            }
            _ => bail!("Node is not a reference"),
        })
    }

    pub(super) fn get_characteristics(&self) -> Result<Characteristics, Error> {
        let mut characteristics = Characteristics::default();

        characteristics.push("Type", self.type_name().context("Failed to get type name")?);

        if let Some(default) = &self.default {
            characteristics
                .push_value("Default", default)
                .context("Could not serialize default")?;
        }

        if let Some(format) = &self.format {
            characteristics.push("Format", format.clone());
        }

        if self.deprecated {
            characteristics.push("Deprecated", "Yes");
        }

        if self.read_only {
            characteristics.push("Read-only", "Yes");
        }

        if self.write_only {
            characteristics.push("Write-only", "Yes");
        }

        // Info for arrays.
        if let Some(ref array_validation) = self.object.array {
            if let Some(n) = array_validation.min_items {
                characteristics.push_display("Min items", n);
            }

            if let Some(n) = array_validation.max_items {
                characteristics.push_display("Max items", n);
            }

            if let Some(b) = array_validation.unique_items {
                characteristics.push_display("Unique items", b);
            }
        }

        // Info for simple enums
        if let NodeKind::SimpleEnum(instance_type) = self.kind {
            characteristics.push("Variants", instance_type_name(instance_type));
        }

        // Examples!
        if !self.examples.is_empty() {
            characteristics.push_markdown(
                "Examples",
                self.examples
                    .iter()
                    .map(|v| serde_yaml::to_string(v).map(|s| format!("`{}`", s.trim())))
                    .collect::<Result<Vec<_>, _>>()
                    .context("Could not serialize examples")?
                    .join("<br>"),
            );
        }

        Ok(characteristics)
    }
}

fn instance_type_name(it: InstanceType) -> &'static str {
    match it {
        InstanceType::Null => "null",
        InstanceType::Boolean => "boolean",
        InstanceType::Array => "array",
        InstanceType::Number => "number",
        InstanceType::String => "string",
        InstanceType::Integer => "integer",
        InstanceType::Object => "object",
    }
}

/// Gets the name of the reference contained in a given schema, when available.
fn get_reference_name(schema: &Schema) -> Result<&str, Error> {
    match schema {
        Schema::Bool(_) => bail!("Boolean schema has no reference"),
        Schema::Object(obj) => obj
            .reference
            .as_ref()
            .map(|r| r.as_str().trim_start_matches("#/definitions/"))
            .context("Object schema has no reference"),
    }
}
