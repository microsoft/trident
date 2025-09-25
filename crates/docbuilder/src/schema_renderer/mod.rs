use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};

use log::info;
use trident_api::schemars::schema::{RootSchema, SchemaObject};

mod characteristics;
pub(crate) mod node;
mod renderer;
mod tera_context;
mod tera_extensions;

use node::SchemaNodeModel;
use renderer::{NodeRenderer, Page};

use self::tera_context::TeraContextFactory;

#[derive(Default, Debug)]
pub(crate) struct SchemaDocSettings {
    /// Whether to create a DevOps wiki order file
    pub devops_wiki: bool,

    /// Whether to use docfx-only features
    pub docfx: bool,
}

pub(crate) struct SchemaDocBuilder {
    /// The schema to document.
    root: RootSchema,

    /// Title of the schema
    title: String,

    /// Renderer
    renderer: NodeRenderer,

    /// Settings
    settings: SchemaDocSettings,
}

impl SchemaDocBuilder {
    /// Create a new schema doc builder with the given settings.
    pub(crate) fn new(root: RootSchema, settings: SchemaDocSettings) -> Result<Self, Error> {
        let title = root
            .schema
            .metadata
            .as_ref()
            .and_then(|m| m.title.as_ref())
            .context("Root schema object must have a title!")?
            .clone();

        Ok(Self {
            renderer: NodeRenderer::new(
                DefinitionMapper::new(&title, root.definitions.keys()),
                TeraContextFactory::new(settings.docfx),
            )?,
            root,
            title,
            settings,
        })
    }

    /// Build the pages for the schema.
    pub(crate) fn build_pages(&self) -> Result<Vec<Page>, Error> {
        let mut pages: Vec<Page> = vec![self
            .build_root_page()
            .context("Failed to build root page")?];
        pages.append(&mut self.build_definition_pages()?);

        if self.settings.devops_wiki {
            pages.push(Page {
                relative_path: PathBuf::from(".order"),
                content: pages
                    .iter()
                    .map(|p| {
                        p.relative_path
                            .with_extension("")
                            .file_name()
                            .expect("The page doe snot contain a filename!")
                            .to_string_lossy()
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        }

        Ok(pages)
    }

    /// Build the root page for the schema.
    fn build_root_page(&self) -> Result<Page, Error> {
        self.build_page(&self.title, &self.root.schema)
    }

    /// Build the pages for the definitions in the schema.
    fn build_definition_pages(&self) -> Result<Vec<Page>, Error> {
        self.root
            .definitions
            .iter()
            .map(|(name, obj)| self.build_page(name, &obj.clone().into_object()))
            .collect()
    }

    /// Build a page for the given schema object.
    fn build_page(&self, id: &str, obj: &SchemaObject) -> Result<Page, Error> {
        info!("Building page for '{}'", id);
        let node = SchemaNodeModel::try_from(obj)
            .context("Failed to convert root schema to node model")?;

        self.renderer
            .render_page(id, node)
            .context(format!("Failed to make page for for '{id}'"))
    }
}

/// Struct to hold the mapping between a definition name and the path to the
/// markdown file that will be generated for it.
pub(crate) struct DefinitionMapper {
    definitions: HashMap<String, PathBuf>,
}

impl DefinitionMapper {
    fn make_path(name: impl AsRef<str>) -> PathBuf {
        let mut path = PathBuf::from(".");
        let name = name.as_ref().replace(' ', "-");
        path.push(name);
        path.set_extension("md");
        path
    }

    fn new(root: impl AsRef<str>, keys: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut definitions = keys
            .into_iter()
            .map(|k| (k.as_ref().to_string(), Self::make_path(k)))
            .collect::<HashMap<String, PathBuf>>();
        definitions.insert(root.as_ref().to_string(), Self::make_path(root));
        Self { definitions }
    }

    fn get_file(&self, name: impl AsRef<str>) -> Option<&Path> {
        self.definitions.get(name.as_ref()).map(|p| p.as_ref())
    }

    fn get_link_from_reference(&self, reference: impl AsRef<str>) -> Option<&Path> {
        self.get_file(reference.as_ref().trim_start_matches("#/definitions/"))
    }
}

fn serde_json_value_friendly(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Array(s) => format!(
            "[{}]",
            s.iter()
                .map(serde_json_value_friendly)
                .collect::<Vec<String>>()
                .join(", ")
        ),
        serde_json::Value::Object(s) => format!(
            "{{ {} }}",
            s.iter()
                .map(|(k, v)| { format!("{}: {}", k, serde_json_value_friendly(v)) })
                .collect::<Vec<String>>()
                .join(", ")
        ),
    }
}
