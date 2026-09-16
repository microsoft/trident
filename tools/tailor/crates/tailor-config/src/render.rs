//! The per-cell render pipeline: gather matched fragments, resolve `$include`, merge the config and
//! tailor fields, interpolate `${…}`, and emit one runnable cell (`meta/docs/2026-06-22-image-definitions.md` §7,
//! §9.3). Pure and deterministic, so the emitted config is a stable golden snapshot.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde_yaml_ng::{Mapping, Value};

use crate::{
    error::ConfigError,
    fragment::{self, LoadedFragment},
    include, interpolate, matrix,
    matrix::AxisTuple,
    merge,
    schema::{BaseSource, ExtraParam, ImageDefinition, OutputSpec},
    types::{OutputFormat, ParamValue},
};

const BASE_FIELD: &str = "base";
const OUTPUTS_FIELD: &str = "outputs";
const RENDERED_DIR: &str = ".rendered";

/// Write a cell's normalized golden snapshot to `<image_dir>/.rendered/<slug>.yaml` (for review and
/// CI blast-radius diffing, `meta/docs/2026-06-22-image-definitions.md` §9.3). Returns the written path.
pub fn write_golden(
    image_dir: &Path,
    slug: &str,
    ic_config: &Value,
) -> Result<PathBuf, ConfigError> {
    let dir = image_dir.join(RENDERED_DIR);
    fs::create_dir_all(&dir).map_err(|source| ConfigError::Write {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(format!("{slug}.yaml"));
    let text = serde_yaml_ng::to_string(ic_config).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
    })?;
    fs::write(&path, text).map_err(|source| ConfigError::Write {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// One fully rendered matrix cell, ready for execution or golden snapshotting.
#[derive(Debug, Clone)]
pub struct RenderedCell {
    /// The cell's axis coordinate (matrix-declared order).
    pub tuple: AxisTuple,
    /// The merged, interpolated Image Customizer config (the `config:` tree).
    pub ic_config: Value,
    /// The single resolved base image source.
    pub base: BaseSource,
    /// The output formats to build for this cell.
    pub outputs: Vec<OutputSpec>,
    /// Local RPM sources (directories or `.repo` files) passed to IC as `--rpm-source`.
    pub rpm_sources: Vec<PathBuf>,
    /// Extra IC command-line flags appended verbatim, concatenated across matched fragments.
    pub extra_params: Vec<ExtraParam>,
    /// Resolved `skip` for this cell (merged from fragment `skip:` fields, last-wins). When `true`,
    /// the cell is dropped from bulk selection unless specifically requested
    /// (`meta/docs/2026-07-22-fragment-skip.md`).
    pub skip: bool,
    /// The `(axis, value)` coordinates that a `-s` selector must pin to keep this cell despite `skip`
    /// (the predicate of the fragment that set `skip: true`); empty when not skipped.
    pub skip_pins: Vec<(String, String)>,
}

/// Render every cell of an image. `$include` paths resolve relative to `image_dir`.
pub fn render_image(
    image: &ImageDefinition,
    image_dir: impl AsRef<Path>,
) -> Result<Vec<RenderedCell>, ConfigError> {
    let image_dir = image_dir.as_ref();
    let fragments = fragment::discover(image_dir, image.matrix.as_ref(), &image.features)?;
    let cells = match &image.matrix {
        Some(axes) => matrix::expand(axes, image.selectors.as_ref())?,
        None if image.selectors.is_some() => {
            return Err(ConfigError::SelectorsWithoutMatrix {
                slug: image.name.clone(),
            });
        }
        None => vec![AxisTuple { values: Vec::new() }],
    };
    cells
        .into_iter()
        .map(|tuple| render_cell(image, image_dir, &fragments, tuple))
        .collect()
}

/// One step of a cell's merge plan: the fragment file, why it applies, and any `$include`d libraries.
#[derive(Debug, Clone)]
pub struct MergeStep {
    /// The fragment's source label, e.g. `by-boot+verity/uki+root.yaml` (or `image.yaml` for the base).
    pub label: String,
    /// Why it applies: `base`, `boot=uki ∧ verity=root`, `mode ∈ {dev,test}`, or `feature <name>`.
    pub reason: String,
    /// Repo-relative `$include` targets this fragment splices in (in document order).
    pub includes: Vec<String>,
}

/// The ordered list of fragment files that merge into one cell — the literal `merge_into` sequence
/// (base first, later steps win), for `tailor explain`. Reuses the same discovery and precedence sort
/// as [`render_image`], so the order cannot drift from a real render.
pub fn merge_plan(
    image: &ImageDefinition,
    image_dir: impl AsRef<Path>,
    axes: &BTreeMap<String, String>,
) -> Result<Vec<MergeStep>, ConfigError> {
    let fragments = fragment::discover(image_dir.as_ref(), image.matrix.as_ref(), &image.features)?;
    Ok(fragments
        .iter()
        .filter(|f| f.applies(axes, &image.features))
        .map(|f| MergeStep {
            label: f.label.clone(),
            reason: f.reason(),
            includes: collect_includes(f.doc.config.as_ref()),
        })
        .collect())
}

/// Collect the `$include` targets referenced anywhere in a fragment's `config:` tree, in order.
fn collect_includes(config: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(value) = config {
        walk_includes(value, &mut out);
    }
    out
}

fn walk_includes(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Mapping(map) => {
            for (key, child) in map {
                if key.as_str() == Some("$include") {
                    if let Some(target) = child.as_str() {
                        out.push(target.to_owned());
                    }
                } else {
                    walk_includes(child, out);
                }
            }
        }
        Value::Sequence(items) => items.iter().for_each(|item| walk_includes(item, out)),
        _ => {}
    }
}

fn render_cell(
    image: &ImageDefinition,
    image_dir: &Path,
    fragments: &[LoadedFragment],
    tuple: AxisTuple,
) -> Result<RenderedCell, ConfigError> {
    let axes: BTreeMap<String, String> = tuple.values.iter().cloned().collect();
    let matched: Vec<&LoadedFragment> = fragments
        .iter()
        .filter(|f| f.applies(&axes, &image.features))
        .collect();

    let params = merge_params(&matched)?;
    let context = interpolate::build_context(&axes, &params)?;

    let mut config = Mapping::new();
    for fragment in &matched {
        let Some(delta) = fragment.doc.config.clone() else {
            continue;
        };
        let mut delta = delta;
        include::resolve_includes(&mut delta, image_dir)?;
        let Value::Mapping(delta) = delta else {
            return Err(ConfigError::InvalidField {
                slug: slug(image, &tuple),
                field: "config",
                detail:
                    "expected a mapping (inline IC config); path-string config is not yet supported"
                        .to_owned(),
            });
        };
        merge::merge_into(&mut config, delta, &fragment.label)?;
    }
    let mut ic_config = Value::Mapping(config);
    interpolate::interpolate_tree(&mut ic_config, &context)?;

    let base = resolve_base(image, &tuple, &matched, &context)?;
    let outputs = resolve_outputs(image, &tuple, &matched)?;
    for output in &outputs {
        validate_output_compression(image, &tuple, output)?;
    }
    let rpm_sources = matched
        .iter()
        .flat_map(|f| f.doc.rpm_sources.clone())
        .collect();
    let extra_params = matched
        .iter()
        .flat_map(|f| f.doc.extra_params.clone())
        .collect();

    // Resolve `skip` last-wins over the matched fragments (base → most-specific). When the winning
    // value is `true`, remember that fragment's predicate coordinates as the pins that can override
    // the skip via `-s` (`meta/docs/2026-07-22-fragment-skip.md`).
    let mut skip = false;
    let mut skip_pins: Vec<(String, String)> = Vec::new();
    for fragment in &matched {
        if let Some(flag) = fragment.doc.skip {
            skip = flag;
            skip_pins = if flag { fragment.pins() } else { Vec::new() };
        }
    }

    Ok(RenderedCell {
        tuple,
        ic_config,
        base,
        outputs,
        rpm_sources,
        extra_params,
        skip,
        skip_pins,
    })
}

fn merge_params(matched: &[&LoadedFragment]) -> Result<IndexMap<String, String>, ConfigError> {
    let mut params: IndexMap<String, String> = IndexMap::new();
    for fragment in matched {
        for (name, value) in &fragment.doc.params {
            let value = param_string(value);
            match params.get(name) {
                Some(existing) if existing != &value => {
                    return Err(ConfigError::ParamConflict {
                        name: name.clone(),
                        existing: existing.clone(),
                        incoming: value,
                    });
                }
                _ => {
                    params.insert(name.clone(), value);
                }
            }
        }
    }
    Ok(params)
}

fn resolve_base(
    image: &ImageDefinition,
    tuple: &AxisTuple,
    matched: &[&LoadedFragment],
    context: &interpolate::Context,
) -> Result<BaseSource, ConfigError> {
    let mut base: Option<Value> = None;
    for fragment in matched {
        if let Some(value) = fragment.doc.base.clone() {
            base = Some(merge::merge_field(
                base,
                value,
                BASE_FIELD,
                &fragment.label,
            )?);
        }
    }
    let Some(mut base) = base else {
        return Err(ConfigError::MissingBase {
            slug: slug(image, tuple),
        });
    };
    interpolate::interpolate_tree(&mut base, context)?;
    ensure_single_base_kind(&base, image, tuple)?;
    deserialize_field(base, image, tuple, BASE_FIELD)
}

/// A base is a `oneOf` (`path` | `oci` | `azureLinux` | `image`). Two fragments merging incompatible
/// base *kinds* without `$set` would otherwise silently keep the first; reject the ambiguity loudly.
fn ensure_single_base_kind(
    base: &Value,
    image: &ImageDefinition,
    tuple: &AxisTuple,
) -> Result<(), ConfigError> {
    const KINDS: [&str; 5] = ["path", "oci", "azureLinux", "ref", "image"];
    let present: Vec<&str> = KINDS
        .into_iter()
        .filter(|kind| base.get(kind).is_some())
        .collect();
    match present.len() {
        1 => Ok(()),
        0 => Err(ConfigError::MissingBase {
            slug: slug(image, tuple),
        }),
        _ => Err(ConfigError::AmbiguousBase {
            slug: slug(image, tuple),
            kinds: present.join(", "),
        }),
    }
}

fn resolve_outputs(
    image: &ImageDefinition,
    tuple: &AxisTuple,
    matched: &[&LoadedFragment],
) -> Result<Vec<OutputSpec>, ConfigError> {
    let mut outputs: Option<Value> = None;
    for fragment in matched {
        if let Some(value) = fragment.doc.outputs.clone() {
            outputs = Some(merge::merge_field(
                outputs,
                value,
                OUTPUTS_FIELD,
                &fragment.label,
            )?);
        }
    }
    match outputs {
        Some(value) => deserialize_field(value, image, tuple, OUTPUTS_FIELD),
        None => Ok(Vec::new()),
    }
}

/// Reject `compression:` on formats where it makes no sense: `cosi` (IC already compresses it),
/// `iso` (compressing the image breaks bootability), and the `pxe-*` outputs (a directory / an
/// already-gzipped tar). Compression is a tailor post-step over a single raw disk image.
fn validate_output_compression(
    image: &ImageDefinition,
    tuple: &AxisTuple,
    output: &OutputSpec,
) -> Result<(), ConfigError> {
    let Some(compression) = output.compression else {
        return Ok(());
    };
    if matches!(
        output.format,
        OutputFormat::Cosi | OutputFormat::Iso | OutputFormat::PxeDir | OutputFormat::PxeTar
    ) {
        return Err(ConfigError::InvalidField {
            slug: slug(image, tuple),
            field: OUTPUTS_FIELD,
            detail: format!(
                "compression `{compression}` is not supported for format `{}` (only raw disk-image \
                 formats: vhd, vhd-fixed, vhdx, qcow2, raw, baremetal-image)",
                output.format
            ),
        });
    }
    Ok(())
}

fn deserialize_field<T: DeserializeOwned>(
    value: Value,
    image: &ImageDefinition,
    tuple: &AxisTuple,
    field: &'static str,
) -> Result<T, ConfigError> {
    serde_yaml_ng::from_value(value).map_err(|source| ConfigError::InvalidField {
        slug: slug(image, tuple),
        field,
        detail: source.to_string(),
    })
}

fn param_string(value: &ParamValue) -> String {
    match value {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Int(i) => i.to_string(),
        ParamValue::Float(f) => f.to_string(),
        ParamValue::Str(s) => s.clone(),
    }
}

fn slug(image: &ImageDefinition, tuple: &AxisTuple) -> String {
    let coordinate = tuple.coordinate();
    if coordinate.is_empty() {
        image.name.clone()
    } else {
        format!("{}_{coordinate}", image.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use tempfile::TempDir;

    use crate::{loader::load_image, types::OutputFormat};

    /// Write `body` to `<root>/<rel>`, creating parent directories as needed.
    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// A small matrix image exercising every render operation: list append, `$remove`, `$replace`,
    /// `$set`, `$include`, and (nested) parameter interpolation across by-arch/by-edition fragments.
    fn mini_image(root: &Path) -> ImageDefinition {
        write(
            root,
            "image.yaml",
            indoc! {"
                name: mini
                matrix:
                  arch: [amd64, arm64]
                  edition: [lite, pro]
                outputs:
                  - format: cosi
                config:
                  os:
                    packages:
                      install:
                        - core
                        - \"${bootPkg}\"
            "},
        );
        write(
            root,
            "by-arch/amd64.yaml",
            "base:\n  path: ./amd64.img\nparams:\n  efiArch: x64\n",
        );
        write(
            root,
            "by-arch/arm64.yaml",
            "base:\n  path: ./arm64.img\nparams:\n  efiArch: aa64\n",
        );
        write(
            root,
            "by-edition/lite.yaml",
            indoc! {"
                params:
                  bootPkg: \"boot-${efiArch}\"
                config:
                  storage:
                    $include: layouts/disk.yaml
            "},
        );
        write(
            root,
            "by-edition/pro.yaml",
            indoc! {"
                outputs:
                  $replace:
                    - format: raw
                params:
                  bootPkg: boot-pro
                base:
                  $set:
                    oci:
                      uri: registry.example/mini:pro
                      platform: \"linux/${arch}\"
                config:
                  os:
                    packages:
                      install:
                        $remove:
                          - core
            "},
        );
        write(root, "layouts/disk.yaml", "bootType: efi\n");
        load_image(root.join("image.yaml")).unwrap()
    }

    fn cell<'a>(cells: &'a [RenderedCell], edition: &str, arch: &str) -> &'a RenderedCell {
        cells
            .iter()
            .find(|c| c.tuple.get("edition") == Some(edition) && c.tuple.get("arch") == Some(arch))
            .unwrap_or_else(|| panic!("no {edition}/{arch} cell"))
    }

    fn install(cell: &RenderedCell) -> Vec<&str> {
        cell.ic_config["os"]["packages"]["install"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect()
    }

    #[test]
    fn compression_parses_and_is_validated_against_the_format() {
        // A raw disk-image format accepts compression; the parsed OutputSpec carries it.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            indoc! {"
                name: comp
                base:
                  path: ./b.img
                outputs:
                  - format: raw
                    compression: zstd
                config:
                  os: { hostname: comp }
            "},
        );
        let image = load_image(tmp.path().join("image.yaml")).unwrap();
        let cells = render_image(&image, tmp.path()).unwrap();
        assert_eq!(
            cells[0].outputs[0].compression,
            Some(crate::types::Compression::Zstd)
        );

        // COSI is already compressed by IC — `compression:` on it is rejected.
        let bad = TempDir::new().unwrap();
        write(
            bad.path(),
            "image.yaml",
            indoc! {"
                name: comp
                base:
                  path: ./b.img
                outputs:
                  - format: cosi
                    compression: zstd
                config:
                  os: { hostname: comp }
            "},
        );
        let image = load_image(bad.path().join("image.yaml")).unwrap();
        let err = render_image(&image, bad.path()).unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::InvalidField {
                    field: "outputs",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn renders_one_cell_per_matrix_point() {
        let tmp = TempDir::new().unwrap();
        let image = mini_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        assert_eq!(cells.len(), 4); // edition[2] × arch[2]
    }

    #[test]
    fn selectors_filter_the_matrix_through_render() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            indoc! {"
                name: sel
                matrix:
                  arch: [amd64, arm64]
                  edition: [lite, pro]
                selectors:
                  include:
                    - { arch: amd64 }                  # both editions on amd64
                    - { arch: arm64, edition: [lite] }  # only lite on arm64 (list value)
                outputs:
                  - format: cosi
                base:
                  path: ./b.img
                config:
                  os: { hostname: sel }
            "},
        );
        let image = load_image(tmp.path().join("image.yaml")).unwrap();
        let cells = render_image(&image, tmp.path()).unwrap();
        let coords: Vec<String> = cells.iter().map(|c| c.tuple.coordinate()).collect();
        // amd64_lite, amd64_pro, arm64_lite — the arm64_pro cell is filtered out.
        assert_eq!(coords, ["amd64_lite", "amd64_pro", "arm64_lite"]);
    }

    #[test]
    fn selectors_without_a_matrix_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            indoc! {"
                name: bad
                selectors:
                  include:
                    - { arch: amd64 }
                base:
                  path: ./b.img
                config:
                  os: { hostname: bad }
            "},
        );
        let image = load_image(tmp.path().join("image.yaml")).unwrap();
        let err = render_image(&image, tmp.path()).unwrap_err();
        assert!(
            matches!(err, ConfigError::SelectorsWithoutMatrix { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn extra_params_concatenate_base_then_fragment() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            indoc! {"
                name: xp
                matrix:
                  arch: [amd64, arm64]
                extraParams:
                  - param: --base-flag
                    value: on
                base:
                  path: ./b.img
                config:
                  os: { hostname: xp }
            "},
        );
        write(
            tmp.path(),
            "by-arch/arm64.yaml",
            indoc! {"
                extraParams:
                  - param: --arm64-only
            "},
        );
        let image = load_image(tmp.path().join("image.yaml")).unwrap();
        let cells = render_image(&image, tmp.path()).unwrap();

        let amd64 = cells
            .iter()
            .find(|c| c.tuple.get("arch") == Some("amd64"))
            .unwrap();
        let arm64 = cells
            .iter()
            .find(|c| c.tuple.get("arch") == Some("arm64"))
            .unwrap();

        let flags = |c: &RenderedCell| -> Vec<(String, Option<String>)> {
            c.extra_params
                .iter()
                .map(|p| (p.param.clone(), p.value.clone()))
                .collect()
        };
        assert_eq!(
            flags(amd64),
            [("--base-flag".to_owned(), Some("on".to_owned()))]
        );
        // base → most-specific: the fragment's flag is appended after the base document's.
        assert_eq!(
            flags(arm64),
            [
                ("--base-flag".to_owned(), Some("on".to_owned())),
                ("--arm64-only".to_owned(), None),
            ]
        );
    }

    #[test]
    fn lite_cell_keeps_path_base_appends_packages_and_resolves_include_and_nested_params() {
        let tmp = TempDir::new().unwrap();
        let image = mini_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        let lite = cell(&cells, "lite", "amd64");

        // by-arch supplies a local path base; the inherited cosi output is unchanged.
        assert!(matches!(&lite.base, BaseSource::Path { .. }));
        assert_eq!(lite.outputs.len(), 1);
        assert_eq!(lite.outputs[0].format, OutputFormat::Cosi);
        // List append + nested interpolation: bootPkg = "boot-${efiArch}", efiArch = x64.
        assert_eq!(install(lite), ["core", "boot-x64"]);
        // $include splices the storage layout as the value of `storage`.
        assert_eq!(lite.ic_config["storage"]["bootType"].as_str(), Some("efi"));
    }

    #[test]
    fn pro_cell_applies_set_base_replace_outputs_and_remove() {
        let tmp = TempDir::new().unwrap();
        let image = mini_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        let pro = cell(&cells, "pro", "arm64");

        // $set overrides the by-arch path base wholesale; ${arch} interpolates into the platform.
        match &pro.base {
            BaseSource::Oci { oci } => assert_eq!(oci.platform.as_deref(), Some("linux/arm64")),
            other => panic!("expected an OCI base, got {other:?}"),
        }
        // $replace swaps the whole output list.
        assert_eq!(pro.outputs.len(), 1);
        assert_eq!(pro.outputs[0].format, OutputFormat::Raw);
        // $remove drops `core`, leaving only the interpolated boot package.
        assert_eq!(install(pro), ["boot-pro"]);
    }

    #[test]
    fn an_image_with_no_base_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            "name: nobase\nconfig:\n  os:\n    hostname: x\n",
        );
        let image = load_image(tmp.path().join("image.yaml")).unwrap();
        let err = render_image(&image, tmp.path()).unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingBase { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn merge_precedence_follows_axis_declaration_order_not_directory_names() {
        // Two axes whose fragments both `$set` the same scalar. The axis declared LAST wins,
        // regardless of how the `by-*` directories sort on disk (by-aa < by-zz alphabetically).
        fn rendered_hostname(first_axis: &str, second_axis: &str) -> String {
            let tmp = TempDir::new().unwrap();
            write(
                tmp.path(),
                "image.yaml",
                &format!(
                    "name: ord\nmatrix:\n  {first_axis}: [x]\n  {second_axis}: [x]\n\
                     outputs:\n  - format: cosi\nbase:\n  path: ./b.img\n\
                     config:\n  os:\n    hostname: from-base\n"
                ),
            );
            write(
                tmp.path(),
                "by-aa/x.yaml",
                "config:\n  os:\n    hostname:\n      $set: from-aa\n",
            );
            write(
                tmp.path(),
                "by-zz/x.yaml",
                "config:\n  os:\n    hostname:\n      $set: from-zz\n",
            );
            let image = load_image(tmp.path().join("image.yaml")).unwrap();
            let cells = render_image(&image, tmp.path()).unwrap();
            cells[0].ic_config["os"]["hostname"]
                .as_str()
                .unwrap()
                .to_owned()
        }
        // Declaring `aa` before `zz` makes `zz` win; reversing the declaration flips the winner —
        // even though the on-disk directory names (by-aa, by-zz) are identical in both cases.
        assert_eq!(rendered_hostname("aa", "zz"), "from-zz");
        assert_eq!(rendered_hostname("zz", "aa"), "from-aa");
    }

    /// Image exercising composite paths, single-axis disjunction, `$unset`, and `$prepend` together
    /// through the full render+merge pipeline.
    fn combo_image(root: &Path) -> ImageDefinition {
        write(
            root,
            "image.yaml",
            indoc! {"
                name: combo
                matrix:
                  boot: [grub, uki]
                  verity: [none, root, usr]
                  mode: [dev, test, prod]
                outputs:
                  - format: cosi
                base:
                  path: ./b.img
                config:
                  os:
                    hostname: base-host
                    selinux:
                      mode: disabled
                  scripts:
                    post:
                      - path: mid.sh
            "},
        );
        // single-axis disjunction (broader) then the singular override (narrower → wins for `dev`).
        write(
            root,
            "by-mode/dev+test.yaml",
            "config:\n  os:\n    hostname:\n      $set: shared\n",
        );
        write(
            root,
            "by-mode/dev.yaml",
            "config:\n  os:\n    hostname:\n      $set: dev\n",
        );
        // $unset removes an inherited key for one axis value.
        write(
            root,
            "by-verity/none.yaml",
            "config:\n  os:\n    selinux: $unset\n",
        );
        // multi-axis composite applies only to the (uki, root) pair.
        write(
            root,
            "by-boot+verity/uki+root.yaml",
            "config:\n  os:\n    label: uki-root\n",
        );
        // $prepend puts a script before the inherited one for uki boots.
        write(
            root,
            "by-boot/uki.yaml",
            "config:\n  scripts:\n    post:\n      $prepend:\n        - path: first.sh\n",
        );
        load_image(root.join("image.yaml")).unwrap()
    }

    fn combo_cell<'a>(
        cells: &'a [RenderedCell],
        boot: &str,
        verity: &str,
        mode: &str,
    ) -> &'a RenderedCell {
        cells
            .iter()
            .find(|c| {
                c.tuple.get("boot") == Some(boot)
                    && c.tuple.get("verity") == Some(verity)
                    && c.tuple.get("mode") == Some(mode)
            })
            .unwrap_or_else(|| panic!("no {boot}/{verity}/{mode} cell"))
    }

    fn scripts(cell: &RenderedCell) -> Vec<&str> {
        cell.ic_config["scripts"]["post"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|s| s["path"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn composite_disjunction_unset_and_prepend_render_together() {
        let tmp = TempDir::new().unwrap();
        let image = combo_image(tmp.path());
        let cells = render_image(&image, tmp.path()).unwrap();
        assert_eq!(cells.len(), 2 * 3 * 3); // boot × verity × mode

        // (uki, root, dev): singular `dev` beats the `dev+test` disjunction; composite label applies;
        // selinux survives (verity != none); $prepend puts first.sh ahead of the inherited mid.sh.
        let a = combo_cell(&cells, "uki", "root", "dev");
        assert_eq!(a.ic_config["os"]["hostname"].as_str(), Some("dev"));
        assert_eq!(a.ic_config["os"]["label"].as_str(), Some("uki-root"));
        assert_eq!(
            a.ic_config["os"]["selinux"]["mode"].as_str(),
            Some("disabled")
        );
        assert_eq!(scripts(a), ["first.sh", "mid.sh"]);

        // (uki, none, test): disjunction wins (no singular for `test`); selinux key removed by $unset;
        // composite does NOT apply (verity != root); $prepend still applies (boot == uki).
        let b = combo_cell(&cells, "uki", "none", "test");
        assert_eq!(b.ic_config["os"]["hostname"].as_str(), Some("shared"));
        assert!(
            !b.ic_config["os"]
                .as_mapping()
                .unwrap()
                .contains_key("selinux")
        );
        assert!(
            !b.ic_config["os"]
                .as_mapping()
                .unwrap()
                .contains_key("label")
        );
        assert_eq!(scripts(b), ["first.sh", "mid.sh"]);

        // (grub, root, prod): no axis override → base hostname; selinux kept; no composite; no prepend.
        let c = combo_cell(&cells, "grub", "root", "prod");
        assert_eq!(c.ic_config["os"]["hostname"].as_str(), Some("base-host"));
        assert!(
            !c.ic_config["os"]
                .as_mapping()
                .unwrap()
                .contains_key("label")
        );
        assert_eq!(scripts(c), ["mid.sh"]);
    }
}
