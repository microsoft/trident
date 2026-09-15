//! Workspace discovery (Cargo model) — `meta/docs/2026-06-22-design.md` §5.3.
//!
//! `tailor` walks up from the current directory to find `tailor.yaml` (the workspace root); every
//! member image (`*/image.yaml` auto-discovered at depth 1, or curated via the `images` catalogue)
//! belongs to that workspace. With no manifest, a lone `image.yaml` is a standalone image.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    error::ConfigError,
    loader::{load_image, load_tool_config},
    schema::{ImageDefinition, ToolConfig},
};

const MANIFEST: &str = "tailor.yaml";
const IMAGE_FILE: &str = "image.yaml";
const ALL_MEMBERS_GLOB: &str = "*";

/// A discovered image: its parsed definition and the directory holding its `image.yaml`.
#[derive(Debug, Clone)]
pub struct DiscoveredImage {
    pub definition: ImageDefinition,
    pub dir: PathBuf,
}

/// A resolved workspace: the root directory, the optional tool config, and the member images.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    /// The parsed `tailor.yaml`, or `None` in standalone mode.
    pub tool: Option<ToolConfig>,
    pub images: Vec<DiscoveredImage>,
}

impl Workspace {
    /// Find an image by name.
    pub fn image(&self, name: &str) -> Option<&DiscoveredImage> {
        self.images.iter().find(|img| img.definition.name == name)
    }
}

/// Find the nearest `tailor.yaml` at or above `start`.
pub fn find_manifest(start: impl AsRef<Path>) -> Option<PathBuf> {
    let mut dir = Some(start.as_ref());
    while let Some(current) = dir {
        let candidate = current.join(MANIFEST);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

/// Discover the workspace containing `start`: a `tailor.yaml` workspace (with member images), or a
/// standalone `image.yaml` if no manifest is found.
pub fn discover(start: impl AsRef<Path>) -> Result<Workspace, ConfigError> {
    let start = start.as_ref();
    if let Some(manifest) = find_manifest(start) {
        let root = manifest.parent().unwrap_or(start).to_path_buf();
        let tool = load_tool_config(&manifest)?;
        let images = discover_members(&root, &tool)?;
        return Ok(Workspace {
            root,
            tool: Some(tool),
            images,
        });
    }
    let definition = load_image(start.join(IMAGE_FILE))?;
    Ok(Workspace {
        root: start.to_path_buf(),
        tool: None,
        images: vec![DiscoveredImage {
            definition,
            dir: start.to_path_buf(),
        }],
    })
}

fn discover_members(root: &Path, tool: &ToolConfig) -> Result<Vec<DiscoveredImage>, ConfigError> {
    let catalogue = tool.images.as_ref();
    let excluded: Vec<String> = catalogue
        .map(|c| c.exclude.iter().map(|e| normalize_member(e)).collect())
        .unwrap_or_default();

    let member_dirs = match catalogue.and_then(|c| c.members.as_ref()) {
        Some(members) => {
            let mut dirs = Vec::new();
            for member in members {
                if normalize_member(member) == ALL_MEMBERS_GLOB {
                    dirs.extend(depth_one_image_dirs(root)?);
                } else {
                    dirs.push(root.join(normalize_member(member)));
                }
            }
            dirs
        }
        None => depth_one_image_dirs(root)?,
    };

    let mut images = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in member_dirs {
        // The `*` glob and an explicit member entry can name the same directory; keep it once.
        if !seen.insert(dir.clone()) {
            continue;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if excluded.contains(&name) {
            continue;
        }
        let image_path = dir.join(IMAGE_FILE);
        if image_path.is_file() {
            images.push(DiscoveredImage {
                definition: load_image(&image_path)?,
                dir,
            });
        }
    }
    if let Some(catalogue) = catalogue {
        for definition in &catalogue.inline {
            images.push(DiscoveredImage {
                definition: definition.clone(),
                dir: root.to_path_buf(),
            });
        }
    }
    Ok(images)
}

/// Immediate subdirectories of `root` that contain an `image.yaml`, sorted for determinism.
fn depth_one_image_dirs(root: &Path) -> Result<Vec<PathBuf>, ConfigError> {
    let mut dirs = Vec::new();
    let entries = fs::read_dir(root).map_err(|source| ConfigError::Read {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| ConfigError::Read {
                path: root.to_path_buf(),
                source,
            })?
            .path();
        if path.is_dir() && path.join(IMAGE_FILE).is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Normalize a catalogue member/exclude entry to a bare path segment (`./webserver/` ⇒ `webserver`).
fn normalize_member(entry: &str) -> String {
    entry
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use tempfile::TempDir;

    /// Write `body` to `<root>/<rel>`, creating parent directories as needed.
    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    const TOOL: &str = indoc! {"
        schemaVersion: 1
        toolchains:
          default: ic
          entries:
            - name: ic
              container: registry.example/imagecustomizer
              version: 1.0.0
    "};

    fn image(name: &str) -> String {
        format!("name: {name}\nbase:\n  path: ./b.img\nconfig:\n  os:\n    hostname: {name}\n")
    }

    #[test]
    fn discovers_the_manifest_and_auto_discovers_member_images() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "tailor.yaml", TOOL);
        write(tmp.path(), "alpha/image.yaml", &image("alpha"));
        write(tmp.path(), "beta/image.yaml", &image("beta"));

        let workspace = discover(tmp.path()).unwrap();
        assert!(workspace.tool.is_some());
        let mut names: Vec<&str> = workspace
            .images
            .iter()
            .map(|i| i.definition.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["alpha", "beta"]);
        assert!(workspace.image("beta").is_some());
    }

    #[test]
    fn glob_and_explicit_member_naming_the_same_dir_is_deduplicated() {
        // `tailor add image` writes `members: ["*", <new>]`; the `*` already covers a depth-1 dir,
        // so the explicit entry must not yield a duplicate image.
        let tmp = TempDir::new().unwrap();
        let manifest = format!("{TOOL}images:\n  members:\n    - \"*\"\n    - alpha\n");
        write(tmp.path(), "tailor.yaml", &manifest);
        write(tmp.path(), "alpha/image.yaml", &image("alpha"));

        let workspace = discover(tmp.path()).unwrap();
        let names: Vec<&str> = workspace
            .images
            .iter()
            .map(|i| i.definition.name.as_str())
            .collect();
        assert_eq!(names, ["alpha"], "alpha must appear exactly once");
    }

    #[test]
    fn find_manifest_walks_up_from_a_nested_directory() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "tailor.yaml", TOOL);
        let nested = tmp.path().join("member/deep/nested");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_manifest(&nested), Some(tmp.path().join("tailor.yaml")));
    }

    #[test]
    fn a_lone_image_without_a_manifest_is_standalone() {
        // No tailor.yaml at or above the tempdir ⇒ standalone mode, one discovered image.
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "image.yaml", &image("solo"));
        let workspace = discover(tmp.path()).unwrap();
        assert!(workspace.tool.is_none());
        assert_eq!(workspace.images.len(), 1);
        assert_eq!(workspace.images[0].definition.name, "solo");
    }
}
