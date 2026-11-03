//! Build script: embed the full version string as the `TAILOR_VERSION` env var, consumed by the
//! CLI for both `--version` and the `version` subcommand.
//!
//! The string is the Cargo (SemVer) version followed by **build metadata** per SemVer §10:
//! `<x.y.z>+<short-commit>.<YYYY-MM-DD>`. Build-metadata identifiers are dot-separated and limited
//! to `[0-9A-Za-z-]`, so a hyphenated ISO date is valid. The date honours `SOURCE_DATE_EPOCH` for
//! reproducible builds; the commit falls back to `unknown` outside a git checkout (e.g. no commits).

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use cargo_metadata::{DependencyKind, MetadataCommand, Node, Package, PackageId};

fn main() {
    emit_version();
    generate_third_party_notices();
}

/// Embed the full version string as `TAILOR_VERSION` (Cargo SemVer + build metadata per SemVer §10:
/// `<x.y.z>+<short-commit>.<YYYY-MM-DD>`), consumed by `--version` and the `version` subcommand.
fn emit_version() {
    let pkg_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let date = build_date();
    println!("cargo::rustc-env=TAILOR_VERSION={pkg_version}+{commit}.{date}");

    // The date input and the checked-out commit are the only things that change the version.
    println!("cargo::rerun-if-env-changed=SOURCE_DATE_EPOCH");
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let head = Path::new(&git_dir).join("HEAD");
        if head.exists() {
            println!("cargo::rerun-if-changed={}", head.display());
        }
        if let Some(branch) = git(&["symbolic-ref", "-q", "--short", "HEAD"]) {
            let reference = Path::new(&git_dir).join("refs").join("heads").join(branch);
            if reference.exists() {
                println!("cargo::rerun-if-changed={}", reference.display());
            }
        }
    }
}

/// Run `git <args>`, returning trimmed stdout on success, or `None` if git is absent/failed.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// The UTC build date as `YYYY-MM-DD`, honouring `SOURCE_DATE_EPOCH` when set.
fn build_date() -> String {
    let epoch = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        })
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(epoch.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Convert days since the Unix epoch (1970-01-01) to a `(year, month, day)` UTC civil date.
/// Howard Hinnant's algorithm: <https://howardhinnant.github.io/date_algorithms.html#civil_from_days>.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let mp = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = day_of_year - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (year + i64::from(month <= 2), month, day)
}

/// Collect the license text of every third-party crate that links into the `tailor` binary and write
/// an aggregated notice to `OUT_DIR/third-party-notices.txt`, embedded at compile time and printed by
/// `tailor notice`. Regenerated whenever `Cargo.lock` changes, so it can never drift from the actual
/// dependency set (no checked-in copy, no drift check needed). `cargo_metadata` is a build-only
/// dependency, so none of this links into the shipped binary.
fn generate_third_party_notices() {
    println!("cargo::rerun-if-changed=Cargo.lock");
    println!("cargo::rerun-if-changed=Cargo.toml");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set for build scripts");
    let dest = Path::new(&out_dir).join("third-party-notices.txt");

    let metadata = MetadataCommand::new()
        .exec()
        .expect("`cargo metadata` for the third-party license notice");
    let resolve = metadata
        .resolve
        .as_ref()
        .expect("cargo metadata resolve graph (run without --no-deps)");

    let packages: BTreeMap<&PackageId, &Package> =
        metadata.packages.iter().map(|p| (&p.id, p)).collect();
    let nodes: BTreeMap<&PackageId, &Node> = resolve.nodes.iter().map(|n| (&n.id, n)).collect();
    let workspace: BTreeSet<&PackageId> = metadata.workspace_members.iter().collect();

    let root = metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == "tailor" && p.source.is_none())
        .map(|p| &p.id)
        .expect("the tailor package is present in metadata");

    // Walk the *normal*-dependency closure (what actually links into the binary): follow only normal
    // edges — never dev- or build-dependency edges — and skip our own workspace crates (covered by
    // tailor's own LICENSE).
    let mut shipped: BTreeSet<&PackageId> = BTreeSet::new();
    let mut seen: BTreeSet<&PackageId> = BTreeSet::new();
    let mut stack: Vec<&PackageId> = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else {
            continue;
        };
        for dep in &node.deps {
            let links_into_binary = dep.dep_kinds.is_empty()
                || dep
                    .dep_kinds
                    .iter()
                    .any(|kind| kind.kind == DependencyKind::Normal);
            if !links_into_binary {
                continue;
            }
            if !workspace.contains(&dep.pkg) {
                shipped.insert(&dep.pkg);
            }
            stack.push(&dep.pkg);
        }
    }

    let mut ordered: Vec<&PackageId> = shipped.into_iter().collect();
    ordered.sort_by(|a, b| {
        let (pa, pb) = (packages[a], packages[b]);
        (pa.name.as_str(), &pa.version).cmp(&(pb.name.as_str(), &pb.version))
    });

    let mut out = String::new();
    out.push_str("THIRD-PARTY SOFTWARE NOTICES\n\n");
    out.push_str(
        "The tailor binary statically links the third-party packages listed below. Each is \
         distributed under the terms of its own license, reproduced in full here. tailor itself is \
         licensed under the MIT License (shown above this section by `tailor notice`).\n",
    );

    for id in ordered {
        let package = packages[id];
        out.push_str("\n================================================================\n");
        let _ = writeln!(out, "{} {}", package.name, package.version);
        if let Some(license) = &package.license {
            let _ = writeln!(out, "SPDX-License-Identifier: {license}");
        }
        if let Some(repository) = &package.repository {
            let _ = writeln!(out, "Repository: {repository}");
        }
        out.push('\n');
        let text = license_texts(package);
        if text.is_empty() {
            out.push_str(
                "(No license file is bundled in this crate; refer to the SPDX identifier above and \
                 the crate's repository.)\n",
            );
        } else {
            out.push_str(&text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }

    fs::write(&dest, out).expect("write third-party notices to OUT_DIR");
}

/// Read the license/notice text a crate ships: its declared `license-file`, plus any
/// `LICENSE*`/`COPYING*`/`NOTICE*`/`UNLICENSE*` files in its root (a crate may ship several, e.g. the
/// MIT and Apache texts of a dual license, plus an Apache `NOTICE`). Exact duplicates are dropped.
fn license_texts(package: &Package) -> String {
    let Some(parent) = package.manifest_path.parent() else {
        return String::new();
    };
    let dir = parent.as_std_path();

    let mut texts: Vec<String> = Vec::new();
    let push_unique = |texts: &mut Vec<String>, text: String| {
        if !text.trim().is_empty() && !texts.contains(&text) {
            texts.push(text);
        }
    };

    if let Some(license_file) = &package.license_file
        && let Ok(text) = fs::read_to_string(dir.join(license_file.as_std_path()))
    {
        push_unique(&mut texts, text);
    }

    if let Ok(entries) = fs::read_dir(dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_license_file(path))
            .collect();
        files.sort();
        for path in files {
            if let Ok(text) = fs::read_to_string(&path) {
                push_unique(&mut texts, text);
            }
        }
    }

    texts.join("\n")
}

/// Whether a filename looks like a license or notice file.
fn is_license_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}
