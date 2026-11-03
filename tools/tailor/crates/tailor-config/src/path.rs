//! Path resolution shared by every consumer of an image definition's filesystem references
//! (`base`, `rpmSources`, …). Relative paths are authored relative to the **image directory** (the
//! folder holding `image.yaml`), so they must be resolved the same way everywhere — the base
//! existence/hash check in `tailor-resolve` and the Image Customizer `--image-file`/`--rpm-source`
//! arguments in `tailor-exec` both go through [`absolutize`]. Keeping a single implementation here
//! prevents the two from drifting (e.g. one resolving against the process CWD, the other against the
//! image directory).

use std::path::{Component, Path, PathBuf};

/// Resolve `path` to an absolute, lexically-normalized path: a relative path (as authored in
/// `image.yaml`, e.g. a `base` or `rpmSources` entry) is joined onto `base_dir` (the image
/// directory), then `.`/`..` components are collapsed. An already-absolute path is returned
/// normalized, ignoring `base_dir`.
pub fn absolutize(path: impl AsRef<Path>, base_dir: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.as_ref().join(path)
    };
    normalize(&joined)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir) | None
                ) {
                    out.pop();
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_base_against_the_image_dir() {
        // Two levels up from `/repo/docs/img` lands in `/repo`, collapsing cleanly.
        assert_eq!(
            absolutize("../../artifacts/core.vhdx", "/repo/docs/img"),
            Path::new("/repo/artifacts/core.vhdx")
        );
    }

    #[test]
    fn leaves_absolute_paths_normalized() {
        assert_eq!(
            absolutize("/abs/base.vhdx", "/images"),
            Path::new("/abs/base.vhdx")
        );
    }

    #[test]
    fn collapses_interior_cur_and_parent_dirs() {
        assert_eq!(absolutize("./a/../b/c", "/root"), Path::new("/root/b/c"));
    }
}
