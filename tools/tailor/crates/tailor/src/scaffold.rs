//! Project scaffolding helpers for `tailor add` (and the pure YAML-editing functions behind them).
//!
//! Editing an existing `tailor.yaml`/`image.yaml` must preserve the user's comments and formatting,
//! so these functions edit the file **text** surgically rather than round-tripping through serde
//! (which would drop comments). They parse only to understand the current state. Each is a pure
//! `&str -> Result<String, String>` transform, unit-tested below.

use tailor_config::{ImageDefinition, ToolConfig};

/// The auto-discovery glob kept in `images.members` so registering one image never hides the others.
const GLOB: &str = "*";

/// Register `member_rel` (a path relative to the workspace root) in the manifest's `images.members`,
/// returning the new manifest text. Idempotent. Preserves auto-discovery of existing images by
/// keeping/seeding the `"*"` glob, and leaves every other line (and its comments) untouched.
pub(crate) fn register_member(manifest: &str, member_rel: &str) -> Result<String, String> {
    let tool: ToolConfig =
        serde_yaml_ng::from_str(manifest).map_err(|e| format!("parse tailor.yaml: {e}"))?;
    let catalogue = tool.images.as_ref();
    if catalogue.is_some_and(|c| !c.inline.is_empty() || !c.exclude.is_empty()) {
        return Err(
            "tailor.yaml uses images.inline/exclude; add the member there by hand".to_owned(),
        );
    }

    let normalized = normalize_member(member_rel);
    let had_members = catalogue.and_then(|c| c.members.as_ref()).is_some();
    let mut members: Vec<String> = catalogue
        .and_then(|c| c.members.clone())
        .unwrap_or_default();
    if members.iter().any(|m| normalize_member(m) == normalized) {
        return Ok(manifest.to_owned()); // already registered
    }
    if !had_members {
        // The manifest relied on auto-discovery; keep finding the other depth-1 images.
        members.push(GLOB.to_owned());
    }
    members.push(normalized);

    Ok(replace_or_append_block(
        manifest,
        "images:",
        &render_images_block(&members),
    ))
}

/// Append a new axis (with a placeholder value, so the matrix stays valid) to an image's `matrix:`
/// block — creating the block if the image has none — returning the new `image.yaml` text.
pub(crate) fn add_axis(image: &str, axis: &str, placeholder: &str) -> Result<String, String> {
    let def: ImageDefinition =
        serde_yaml_ng::from_str(image).map_err(|e| format!("parse image.yaml: {e}"))?;
    if def.matrix.as_ref().is_some_and(|m| m.contains_key(axis)) {
        return Err(format!("axis `{axis}` already exists"));
    }

    let lines: Vec<&str> = image.lines().collect();
    if def.matrix.is_some() {
        let start = lines
            .iter()
            .position(|l| is_top_level_key(l, "matrix:"))
            .ok_or("matrix: block not found")?;
        let block_end = block_end(&lines, start);
        let indent = detect_indent(&lines, start, block_end);
        let axis_line =
            format!("{indent}{axis}: [{placeholder}]  # TODO: replace with real values");
        // Insert after the last non-blank line of the matrix block.
        let insert_at = (start + 1..block_end)
            .rev()
            .find(|&j| !lines[j].trim().is_empty())
            .map_or(block_end, |j| j + 1);
        Ok(splice(&lines, insert_at, &axis_line))
    } else {
        let name = lines
            .iter()
            .position(|l| is_top_level_key(l, "name:"))
            .ok_or("name: field not found")?;
        let new_block =
            format!("\nmatrix:\n  {axis}: [{placeholder}]  # TODO: replace with real values");
        Ok(splice(&lines, name + 1, &new_block))
    }
}

/// Strip a member entry to a bare, comparable path (`./web/` -> `web`).
fn normalize_member(entry: &str) -> String {
    entry
        .trim()
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned()
}

fn render_images_block(members: &[String]) -> String {
    let mut out = String::from("images:\n  members:\n");
    for member in members {
        if member == GLOB {
            out.push_str("    - \"*\"\n");
        } else {
            out.push_str("    - ");
            out.push_str(member);
            out.push('\n');
        }
    }
    out
}

/// Whether `line` is a top-level (column 0) `key:` line matching `header` (e.g. `"images:"`).
fn is_top_level_key(line: &str, header: &str) -> bool {
    line.starts_with(header)
}

/// Whether `line` begins a new top-level construct (column 0, non-blank) — i.e. ends a block.
fn is_top_level_line(line: &str) -> bool {
    !line.is_empty() && !line.starts_with([' ', '\t'])
}

/// The index just past the block that starts at `start` (its first following top-level line, or EOF).
fn block_end(lines: &[&str], start: usize) -> usize {
    (start + 1..lines.len())
        .find(|&j| is_top_level_line(lines[j]))
        .unwrap_or(lines.len())
}

/// The leading whitespace of the first entry in a block (defaults to two spaces for an empty block).
fn detect_indent(lines: &[&str], start: usize, end: usize) -> String {
    (start + 1..end)
        .map(|j| lines[j])
        .find(|l| !l.trim().is_empty())
        .map_or_else(
            || "  ".to_owned(),
            |l| l[..l.len() - l.trim_start().len()].to_owned(),
        )
}

/// Rebuild the text with `insert` spliced in as a new line at index `at`. Ends with a newline.
fn splice(lines: &[&str], at: usize, insert: &str) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == at {
            out.push_str(insert);
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    if at >= lines.len() {
        out.push_str(insert);
        out.push('\n');
    }
    out
}

/// Replace the existing `header` block with `block`, or append `block` if the header is absent.
/// `block` must be a complete, newline-terminated block.
fn replace_or_append_block(text: &str, header: &str, block: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if let Some(start) = lines.iter().position(|l| is_top_level_key(l, header)) {
        let end = block_end(&lines, start);
        let mut out = String::new();
        for line in &lines[..start] {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(block);
        for line in &lines[end..] {
            out.push_str(line);
            out.push('\n');
        }
        out
    } else {
        let mut out = text.to_owned();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n'); // blank-line separator
        out.push_str(block);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "\
# a comment we must keep
schemaVersion: 1
toolchains:
  default: ic
  entries:
    - name: ic
      container: registry.example/ic
";

    fn members_of(manifest: &str) -> Vec<String> {
        let tool: ToolConfig = serde_yaml_ng::from_str(manifest).unwrap();
        tool.images.unwrap().members.unwrap()
    }

    #[test]
    fn register_member_appends_a_block_and_seeds_the_glob() {
        let out = register_member(MANIFEST, "./web/").unwrap();
        assert!(
            out.contains("# a comment we must keep"),
            "comments preserved"
        );
        assert_eq!(members_of(&out), ["*", "web"]);
        // Re-parses as a valid manifest.
        let _: ToolConfig = serde_yaml_ng::from_str(&out).unwrap();
    }

    #[test]
    fn register_member_is_idempotent() {
        let once = register_member(MANIFEST, "web").unwrap();
        let twice = register_member(&once, "web").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn register_member_respects_an_existing_explicit_list_without_adding_the_glob() {
        let manifest = format!("{MANIFEST}images:\n  members:\n    - app\n");
        let out = register_member(&manifest, "db").unwrap();
        assert_eq!(
            members_of(&out),
            ["app", "db"],
            "explicit curation preserved, no glob added"
        );
    }

    #[test]
    fn add_axis_appends_to_an_existing_matrix() {
        let image = "\
name: img
matrix:
  variant: [a, b]
  arch: [amd64, arm64]
outputs:
  - format: cosi
base:
  path: ./b.img
";
        let out = add_axis(image, "release", "TODO").unwrap();
        let def: ImageDefinition = serde_yaml_ng::from_str(&out).unwrap();
        let matrix = def.matrix.unwrap();
        let axes: Vec<&str> = matrix.keys().map(String::as_str).collect();
        assert_eq!(
            axes,
            ["variant", "arch", "release"],
            "appended last, order preserved"
        );
        assert!(out.contains("outputs:"), "later sections untouched");
    }

    #[test]
    fn add_axis_creates_a_matrix_when_the_image_has_none() {
        let image = "name: img\nbase:\n  path: ./b.img\n";
        let out = add_axis(image, "variant", "TODO").unwrap();
        let def: ImageDefinition = serde_yaml_ng::from_str(&out).unwrap();
        let matrix = def.matrix.unwrap();
        let axes: Vec<&str> = matrix.keys().map(String::as_str).collect();
        assert_eq!(axes, ["variant"]);
    }

    #[test]
    fn add_axis_rejects_a_duplicate() {
        let image = "name: img\nmatrix:\n  variant: [a]\nbase:\n  path: ./b.img\n";
        add_axis(image, "variant", "TODO").unwrap_err();
    }
}
