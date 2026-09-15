use std::{fs, io, path::Path};

const REPODATA_DIR: &str = "repodata";

pub fn build_rpm_farm(source: &Path, dest: &Path) -> Result<(), io::Error> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;
    copy_tree(source, source, dest)
}

fn copy_tree(root: &Path, current: &Path, dest: &Path) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() && entry.file_name() == REPODATA_DIR {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let target = dest.join(relative);
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
            copy_tree(root, &path, dest)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            clone_file(&path, &target)?;
        }
    }
    Ok(())
}

fn clone_file(source: &Path, dest: &Path) -> io::Result<()> {
    if reflink_copy::reflink(source, dest).is_ok() {
        return Ok(());
    }
    match fs::hard_link(source, dest) {
        Ok(()) => Ok(()),
        Err(_) => fs::copy(source, dest).map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, path::Path};

    use tempfile::TempDir;

    #[test]
    fn clones_files_and_skips_repodata() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("rpms");
        let nested = source.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(source.join("a.rpm"), b"a").unwrap();
        fs::write(nested.join("b.txt"), b"b").unwrap();
        fs::create_dir_all(source.join(REPODATA_DIR)).unwrap();
        fs::write(source.join(REPODATA_DIR).join("primary.xml"), b"metadata").unwrap();

        let dest = temp.path().join("farm");
        build_rpm_farm(&source, &dest).unwrap();

        assert_eq!(fs::read(dest.join("a.rpm")).unwrap(), b"a");
        assert_eq!(fs::read(dest.join("nested/b.txt")).unwrap(), b"b");
        assert!(!dest.join(REPODATA_DIR).exists());
    }

    #[test]
    fn rebuild_removes_existing_destination() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("new.rpm"), b"new").unwrap();
        let dest = temp.path().join("farm");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("stale.rpm"), b"stale").unwrap();

        build_rpm_farm(&source, &dest).unwrap();

        assert_eq!(fs::read(dest.join("new.rpm")).unwrap(), b"new");
        assert!(!Path::new(&dest.join("stale.rpm")).exists());
    }
}
