use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Error};
use log::debug;
use regex::Regex;
use serde_json::Value;
use tera::{Context as TeraCxt, Tera};
use trident_api::schemars::schema::SingleOrVec;

use super::{
    characteristics::Characteristics,
    node::{NodeKind, SchemaNodeModel},
    tera_context::TeraContextFactory,
    DefinitionMapper,
};

/// A page of documentation.
pub(crate) struct Page {
    pub relative_path: PathBuf,
    pub content: String,
}

/// Node Rendering object.
///
/// This struct is responsible for rendering a page for a node.
pub(super) struct NodeRenderer {
    tera: Tera,
    definitions: DefinitionMapper,
    context_factory: TeraContextFactory,
}

impl NodeRenderer {
    pub(super) fn new(
        definitions: DefinitionMapper,
        context_factory: TeraContextFactory,
    ) -> Result<Self, Error> {
        let mut tera = Tera::new(
            PathBuf::from(file!())
                .parent()
                .unwrap()
                .join("templates/**/*")
                .to_str()
                .context("Failed to get template path")?,
        )
        .context("Failed to load templates")?;

        tera.register_filter(
            "render_characteristic",
            super::tera_extensions::render_characteristic,
        );

        tera.register_filter("header_level", super::tera_extensions::header_level);

        Ok(Self {
            tera,
            definitions,
            context_factory,
        })
    }

    /// Render a full page for this node.
    ///
    /// Only works for independent nodes, i.e. objects (structs) and enums.
    pub(super) fn render_page(&self, id: &str, node: SchemaNodeModel) -> Result<Page, Error> {
        let body = match node.kind {
            NodeKind::Object => self.render_object(id, node),
            NodeKind::Enum => self.render_enum(id, node),
            NodeKind::SimpleEnum(_) => self.render_simple_enum(id, node),
            s => bail!("Unsupported top-level schema type: {:?}", s),
        }
        .context(format!("Failed to render documentation for '{id}'",))?;

        Ok(Page {
            relative_path: self
                .definitions
                .get_file(id)
                .context(format!("Failed to get file path for '{id}'"))?
                .to_path_buf(),
            content: body,
        })
    }

    /// Render a page for this node, assuming it's an object.
    fn render_object(&self, id: &str, node: SchemaNodeModel) -> Result<String, Error> {
        debug!("Rendering object: {}", id);
        let mut context = self.global_context();
        context.insert("title", id);
        context.insert("description", &node.description);
        context.insert(
            "characteristics",
            &node
                .get_characteristics()
                .context(format!("Failed to get characteristics for '{id}'",))?,
        );

        let obj_data = node
            .object
            .object
            .as_ref()
            .context("Node is not an object")?;

        // Generate list of properties.
        let mut properties = obj_data
            .properties
            .iter()
            .map(|(name, schema)| {
                let schema = schema.clone().into_object();
                let node = SchemaNodeModel::try_from(&schema).context(format!(
                    "Failed to convert schema for property '{name}' of '{id}' to node model"
                ))?;

                let required = obj_data.required.contains(name);

                let mut context = self.global_context();
                context.insert("name", name);
                context.insert("required", &required);
                context.insert("type", "property");
                context.insert("level", &3); // How many #'s to use for the header.

                let body = self
                    .render_as_section(node, context)
                    .context(format!("Failed to render property '{name}' for '{id}'",))?;

                Ok((required, body))
            })
            .collect::<Result<Vec<(bool, String)>, Error>>()
            .context("Failed to render properties")?;

        // Sort properties by required, then name.
        properties.sort_by_key(|(required, name)| (!*required, name.clone()));

        // Leave only the body of the property.
        let properties = properties
            .into_iter()
            .map(|(_, body)| body)
            .collect::<Vec<String>>();

        context.insert("properties", &properties);

        self.tera
            .render("object.md.jinja2", &context)
            .context("Failed to render object")
            .map(|s| {
                let re = Regex::new(r"\n{3,}").unwrap();
                re.replace_all(&s, "\n\n").to_string()
            })
    }

    /// Render a page for this node, assuming it's an enum.
    fn render_enum(&self, id: &str, node: SchemaNodeModel) -> Result<String, Error> {
        debug!("Rendering enum: {}", id);
        let mut context = self.global_context();
        context.insert("title", id);
        context.insert("description", &node.description);
        context.insert(
            "characteristics",
            &node
                .get_characteristics()
                .context(format!("Failed to get characteristics for '{id}'",))?,
        );

        let variants = node
            .object
            .subschemas
            .context("Node does not contain subschemas")?
            .one_of
            .context("Node does not contain 'oneOf'")?
            .into_iter()
            .enumerate()
            .map(|(index, schema)| {
                let variant = SchemaNodeModel::try_from(schema.into_object()).context(format!(
                    "Failed to convert schema for variant of '{id}' to node model"
                ))?;

                let mut context = self.global_context();

                let name = match &variant.name {
                    Some(name) => name.clone(),
                    None => format!("variant-{}", index + 1),
                };

                context.insert("name", &name);
                context.insert("type", "variant");
                context.insert("level", &3); // How many #'s to use for the header.

                let body = self
                    .render_as_section(variant, context)
                    .context(format!("Failed to render variant for '{id}'",))?;

                Ok(body)
            })
            .collect::<Result<Vec<String>, Error>>()
            .context("Failed to render variants")?;

        // Check that we have variants!
        ensure!(!variants.is_empty(), "Enum '{id}' has no variants");

        context.insert("variants", &variants);

        self.tera
            .render("enum.md.jinja2", &context)
            .context("Failed to render enum")
            .map(|s| {
                let re = Regex::new(r"\n{3,}").unwrap();
                re.replace_all(&s, "\n\n").to_string()
            })
    }

    /// Render a page for this node, assuming it's a simple enum.
    fn render_simple_enum(&self, id: &str, node: SchemaNodeModel) -> Result<String, Error> {
        debug!("Rendering simple enum: {}", id);
        let mut context = self.global_context();
        context.insert("title", id);
        context.insert("description", &node.description);
        context.insert(
            "characteristics",
            &node
                .get_characteristics()
                .context(format!("Failed to get characteristics for '{id}'",))?,
        );

        let variants = node
            .object
            .enum_values
            .context("Node does not contain enum values")?
            .into_iter()
            .map(|value| super::serde_json_value_friendly(&value))
            .collect::<Vec<String>>();

        ensure!(!variants.is_empty(), "Enum '{id}' has no variants");

        context.insert("variants", &variants);

        self.tera
            .render("simple_enum.md.jinja2", &context)
            .context("Failed to render simple enum")
            .map(|s| {
                let re = Regex::new(r"\n{3,}").unwrap();
                re.replace_all(&s, "\n\n").to_string()
            })
    }

    /// Get a new context from the renderer's context factory.
    fn global_context(&self) -> TeraCxt {
        self.context_factory.global_context()
    }

    fn render_as_section(
        &self,
        node: SchemaNodeModel,
        mut context: TeraCxt,
    ) -> Result<String, Error> {
        debug!(
            "Rendering node of type '{:?}' as section with context: {:?}",
            node.kind, context
        );

        context.insert("description", &node.description);

        // Get the template to use for this node.
        let template = match node.kind {
            NodeKind::DefinitionReference => "sections/reference.md.jinja2",
            NodeKind::Reference => "sections/reference.md.jinja2",
            NodeKind::SimpleObject => "sections/simple_object.md.jinja2",
            NodeKind::Map(_) => "sections/map.md.jinja2",
            NodeKind::Number => "sections/field.md.jinja2",
            NodeKind::Integer => "sections/field.md.jinja2",
            NodeKind::String => "sections/field.md.jinja2",
            NodeKind::Array => "sections/array.md.jinja2",
            NodeKind::Boolean => "sections/field.md.jinja2",
            NodeKind::Null => "sections/field.md.jinja2",
            NodeKind::WrapperEnum(_) => "sections/wrapper_enum.md.jinja2",
            NodeKind::Object => "sections/object.md.jinja2",
            k => {
                context.insert("todo", &format!("context for {:?}", k));
                "sections/field.md.jinja2"
            } // NodeKind::Object | NodeKind::Enum | NodeKind::SimpleEnum(_) => {
              //     bail!(
              //         "Node cannot be rendered as section. It is not a simple type: {:?}",
              //         node.kind
              //     )
              // }
        };

        debug!("Using template: {}", template);

        let mut characteristics = node
            .get_characteristics()
            .context("Failed to get characteristics")?;

        // Populate the context with the data for this node.
        self.section_customize(node, &mut context, &mut characteristics)?;

        // Insert the customized characteristics into the context.
        context.insert("characteristics", &characteristics);

        self.tera.render(template, &context).with_context(|| {
            format!(
                "Failed to render property {} with template {template}",
                context
                    .get("name")
                    .unwrap_or(&Value::String("unknown".into())),
            )
        })
    }

    /// Populate the context & characteristics with specific data for this node based on its kind.
    ///
    /// Calls the corresponding `section_customize_*` method.
    fn section_customize(
        &self,
        node: SchemaNodeModel,
        context: &mut TeraCxt,
        characteristics: &mut Characteristics,
    ) -> Result<(), Error> {
        match node.kind {
            NodeKind::DefinitionReference => {
                self.section_customize_definition_reference(node, context, characteristics)
            }
            NodeKind::SimpleObject => {
                self.section_customize_simple_object(node, context, characteristics)
            }
            NodeKind::Map(_) => self.section_customize_map(node, context, characteristics),
            NodeKind::Number => self.section_customize_number(node, context, characteristics),
            NodeKind::Integer => self.section_customize_integer(node, context, characteristics),
            NodeKind::String => self.section_customize_string(node, context, characteristics),
            NodeKind::Array => self.section_customize_array(node, context, characteristics),
            NodeKind::Boolean => self.section_customize_boolean(node, context, characteristics),
            NodeKind::Reference => self.section_customize_reference(node, context, characteristics),
            NodeKind::Null => self.section_customize_null(node, context, characteristics),
            NodeKind::Object => self.section_customize_object(node, context, characteristics),
            NodeKind::WrapperEnum(_) => {
                self.section_customize_wrapper_enum(node, context, characteristics)
            }
            NodeKind::Enum | NodeKind::SimpleEnum(_) => {
                bail!(
                    "Node cannot be rendered as attribute. It is not a simple type: {:?}",
                    node.kind
                )
            }
        }
        .context("Failed to render node as attribute".to_string())?;

        Ok(())
    }

    /// For enums that contain exactly one variant.
    ///
    /// These are generally wrappers, so we want to expose the underlying type, instead of the enum.
    fn section_customize_wrapper_enum(
        &self,
        node: SchemaNodeModel,
        context: &mut TeraCxt,
        characteristics: &mut Characteristics,
    ) -> Result<(), Error> {
        context.insert("todo", "context for wrapper enum");
        let inner = {
            let mut vector = node.object.enum_values.context("Node is not an enum")?;
            ensure!(
                vector.len() == 1,
                "Node is not an enum with exactly one variant"
            );
            vector
                .pop()
                .context("Node is not an enum with exactly one variant")?
        };

        let value = serde_yaml::to_string(&inner)
            .context("Failed to serialize inner enum")?
            .trim()
            .to_string();

        // If the value contains a newline, it's a multi-line value, so we need to render it as
        // a code block. Otherwise, we can just render it as a characteristic.
        if value.contains('\n') {
            context.insert(
                "inner",
                &serde_yaml::to_string(&inner).context("Failed to serialize inner enum")?,
            );
        } else {
            characteristics.push("Value", value);
        }

        Ok(())
    }

    fn section_customize_object(
        &self,
        node: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        debug!(
            "Customizing section of type '{:?}' named '{}'",
            node.kind,
            node.name.as_deref().unwrap_or("unknown")
        );

        // The indentation level for this object.
        let level = context
            .get("level")
            .and_then(|v| v.as_u64())
            .context(format!(
                "Failed to get title level for object '{:?}'",
                node.name
            ))?;

        let obj_data = node
            .object
            .object
            .as_ref()
            .context("Node is not an object")?;

        // Generate list of properties.
        let mut properties = obj_data
            .properties
            .iter()
            .map(|(name, schema)| {
                let schema = schema.clone().into_object();
                let node = SchemaNodeModel::try_from(&schema).context(format!(
                    "Failed to convert schema for property '{name}' of object to node model"
                ))?;

                let required = obj_data.required.contains(name);

                let mut context = self.global_context();
                context.insert("name", name);
                context.insert("required", &required);
                context.insert("type", "property");
                context.insert("level", &(level + 1));

                let body = self
                    .render_as_section(node, context)
                    .context(format!("Failed to render property '{name}'",))?;

                Ok((required, body))
            })
            .collect::<Result<Vec<(bool, String)>, Error>>()
            .context("Failed to render properties")?;

        // Sort properties by required, then name.
        properties.sort_by_key(|(required, name)| (!*required, name.clone()));

        // Leave only the body of the property.
        let properties = properties
            .into_iter()
            .map(|(_, body)| body)
            .collect::<Vec<String>>();

        context.insert("properties", &properties);

        Ok(())
    }

    fn section_customize_simple_object(
        &self,
        _: SchemaNodeModel,
        _: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn section_customize_map(
        &self,
        node: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        debug!(
            "Customizing section of type '{:?}' named '{}'",
            node.kind,
            node.name.as_deref().unwrap_or("unknown")
        );

        let additional_properties = node
            .object
            .object
            .context("Node is not an object")?
            .additional_properties
            .context("Node is not an object with additional properties")?
            .into_object();

        let items = SchemaNodeModel::try_from(additional_properties)
            .context("Failed to convert additional properties to node")?;

        context.insert(
            "contents",
            &self
                .render_as_section(items, self.global_context())
                .context("Failed to render array item definition")?,
        );

        Ok(())
    }

    fn section_customize_array(
        &self,
        node: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        debug!(
            "Customizing section of type '{:?}' named '{}'",
            node.kind,
            node.name.as_deref().unwrap_or("unknown")
        );

        let array_validarion = *node.object.array.context("Node is not an array")?;
        let items = array_validarion
            .items
            .context("Array has no item definition")?;
        let items = SchemaNodeModel::try_from(match items {
            SingleOrVec::Single(schema) => schema.into_object(),
            SingleOrVec::Vec(_) => bail!("Multiple item definitions not supported"),
        })
        .context("Failed to convert item to node")?;

        context.insert(
            "contents",
            &self
                .render_as_section(items, self.global_context())
                .context("Failed to render array item definition")?,
        );

        Ok(())
    }

    fn section_customize_boolean(
        &self,
        _: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        context.insert("todo", "context for boolean");
        Ok(())
    }

    fn section_customize_null(
        &self,
        _: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        context.insert("todo", "context for null");
        Ok(())
    }

    fn section_customize_number(
        &self,
        _: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        context.insert("todo", "context for number");
        Ok(())
    }

    fn section_customize_integer(
        &self,
        _: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        context.insert("todo", "context for integer");
        Ok(())
    }

    fn section_customize_string(
        &self,
        _: SchemaNodeModel,
        context: &mut TeraCxt,
        _: &mut Characteristics,
    ) -> Result<(), Error> {
        context.insert("todo", "context for string");
        Ok(())
    }

    fn section_customize_reference(
        &self,
        node: SchemaNodeModel,
        _: &mut TeraCxt,
        characteristics: &mut Characteristics,
    ) -> Result<(), Error> {
        debug!(
            "Customizing section of type '{:?}' named '{}'",
            node.kind,
            node.name.as_deref().unwrap_or("unknown")
        );

        let ref_name = node
            .get_reference()
            .context("Failed to get reference name")?;
        characteristics.push_markdown(
            "Link",
            format!(
                "[{ref_name}]({})",
                self.definitions
                    .get_link_from_reference(&ref_name)
                    .context(format!("Failed to get link for reference '{ref_name}'"))?
                    .to_string_lossy(),
            ),
        );

        Ok(())
    }

    fn section_customize_definition_reference(
        &self,
        node: SchemaNodeModel,
        _: &mut TeraCxt,
        characteristics: &mut Characteristics,
    ) -> Result<(), Error> {
        debug!(
            "Customizing section of type '{:?}' named '{}'",
            node.kind,
            node.name.as_deref().unwrap_or("unknown")
        );

        let ref_name = node
            .get_reference()
            .context("Failed to get reference name")?;
        characteristics.push_markdown(
            "Link",
            format!(
                "[{ref_name}]({})",
                self.definitions
                    .get_link_from_reference(&ref_name)
                    .context(format!("Failed to get link for reference '{ref_name}'"))?
                    .to_string_lossy(),
            ),
        );

        Ok(())
    }
}
