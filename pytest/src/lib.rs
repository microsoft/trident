use core::panic;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use serde::Serialize;

/// Used by `inventory` to collect test case metadata and process them on per
/// crate basis in `generate_pytest_wrappers`.
#[derive(Default, Debug)]
pub struct TestCaseMetadata<'a> {
    pub module: &'a str,
    pub function: &'a str,
    pub negative: bool,
    pub xfail: Option<&'a str>,
    pub skip: Option<&'a str>,
    pub feature: &'a str,
    pub type_: &'a str,
}

#[derive(Serialize, Default, Debug)]
#[serde(transparent)]
struct Manifest {
    crates: HashMap<String, Module>,
}

/// Represents a rust module.
#[derive(Serialize, Default, Debug)]
struct Module {
    /// Test cases in this module.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    test_cases: HashMap<String, TestCaseInfo>,

    /// Submodules of this module.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    submodules: HashMap<String, Module>,
}

/// Represents a specific test case.
#[derive(Serialize, Default, Debug)]
struct TestCaseInfo {
    /// Pytest markers to apply to this test case.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    markers: Vec<String>,

    /// States whether the test case should be marked as expected to fail.
    #[serde(skip_serializing_if = "Option::is_none")]
    xfail: Option<String>,

    /// States whether the test case should be skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    skip: Option<String>,
}

// Registers `TestCaseMetadata` with `inventory` for further processing.
inventory::collect!(TestCaseMetadata<'static>);

/// Processes all entries registered with `inventory` and produces ft.json
///
/// The function iterates over all submitted test cases and builds a tree of
/// `Module` objects to recursively express rust's module hierarchy.
///
/// This tree is then serialized to JSON and written to `ft.json` in the
/// `functional_tests` directory.
pub fn generate_functional_test_manifest() {
    // Write the output to the `functional_tests/` directory.
    // `conftest.py` has a custom pytest item collector for this file.
    std::fs::write(
        get_functional_test_dir().join("ft.json"),
        serde_json::to_string_pretty(&generate_manifest()).unwrap(),
    )
    .unwrap();
}

fn get_functional_test_dir() -> PathBuf {
    let func_test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("functional_tests")
        .canonicalize()
        .expect("failed to canonicalize functional_tests dir");

    if !func_test_dir.exists() {
        panic!(
            "Could not find functional_tests directory in {}",
            func_test_dir.display()
        );
    }

    func_test_dir
}

fn generate_manifest() -> Manifest {
    // This will contain a map from crate name to the module structure.
    let mut manifest = Manifest::default();

    inventory::iter::<TestCaseMetadata>().for_each(|item| {
        // Create an iterator over the module path, splitting on `::`.
        let mut module_path_iter = item.module.split("::");

        // The first element is the crate name.
        let rust_crate = module_path_iter.next().unwrap();

        // Get the module for the current crate.
        let mut module = manifest.crates.entry(rust_crate.to_string()).or_default();

        // Recursively navigate to the module for the current test case.
        for rust_module in module_path_iter {
            module = module
                .submodules
                .entry(rust_module.to_string())
                .or_default();
        }

        // Add the test case to the module.
        module.test_cases.insert(
            item.function.to_string(),
            TestCaseInfo {
                xfail: item.xfail.map(|s| s.to_string()),
                skip: item.skip.map(|s| s.to_string()),
                markers: make_markers(item),
            },
        );
    });

    manifest
}

const DEFAULT_TYPE: &str = "functional";
const DEFAULT_FEATURE: &str = "core";
const POSITIVE_STR: &str = "positive";
const NEGATIVE_STR: &str = "negative";

fn make_markers(item: &TestCaseMetadata) -> Vec<String> {
    [
        if item.type_.is_empty() || item.type_ == DEFAULT_TYPE {
            DEFAULT_TYPE
        } else {
            panic!("Unsupported test type: '{}'.", item.type_);
        },
        if item.negative {
            NEGATIVE_STR
        } else {
            POSITIVE_STR
        },
        match item.feature {
            "" => DEFAULT_FEATURE,
            feature => feature,
        },
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_markers() {
        let markers = make_markers(&TestCaseMetadata {
            module: "foo::bar::baz",
            function: "test_foo",
            negative: false,
            xfail: None,
            skip: None,
            feature: "",
            type_: "",
        });
        assert_eq!(markers, vec![DEFAULT_TYPE, POSITIVE_STR, DEFAULT_FEATURE]);

        let markers = make_markers(&TestCaseMetadata {
            module: "foo::bar::baz",
            function: "test_foo",
            negative: true,
            xfail: None,
            skip: None,
            feature: "",
            type_: "",
        });
        assert_eq!(markers, vec![DEFAULT_TYPE, NEGATIVE_STR, DEFAULT_FEATURE]);

        let markers = make_markers(&TestCaseMetadata {
            module: "foo::bar::baz",
            function: "test_foo",
            negative: false,
            xfail: None,
            skip: None,
            feature: "foo",
            type_: "",
        });
        assert_eq!(markers, vec![DEFAULT_TYPE, POSITIVE_STR, "foo"]);

        let markers = make_markers(&TestCaseMetadata {
            module: "foo::bar::baz",
            function: "test_foo",
            negative: false,
            xfail: None,
            skip: None,
            feature: "",
            type_: DEFAULT_TYPE,
        });
        assert_eq!(markers, vec![DEFAULT_TYPE, POSITIVE_STR, DEFAULT_FEATURE]);
    }

    #[test]
    #[should_panic(expected = "Unsupported test type: 'unexpected-type'.")]
    fn test_make_markers_unsupported_type() {
        make_markers(&TestCaseMetadata {
            module: "foo::bar::baz",
            function: "test_foo",
            negative: false,
            xfail: None,
            skip: None,
            feature: "",
            type_: "unexpected-type",
        });
    }

    #[test]
    fn test_get_functional_test_dir() {
        let func_test_dir = get_functional_test_dir();
        assert!(func_test_dir.exists());
        assert!(func_test_dir.is_dir());
    }

    inventory::submit! {
        TestCaseMetadata {
            module: "pytest::pytest_gen",
            function: "test_foo",
            negative: false,
            xfail: None,
            skip: None,
            feature: "",
            type_: "",
        }
    }

    #[test]
    fn test_generate_manifest() {
        let manifest = generate_manifest();
        assert_eq!(manifest.crates.len(), 1);
        assert_eq!(manifest.crates["pytest"].submodules.len(), 1);
        assert_eq!(
            manifest.crates["pytest"].submodules["pytest_gen"]
                .test_cases
                .len(),
            1
        );
        assert_eq!(
            manifest.crates["pytest"].submodules["pytest_gen"].test_cases["test_foo"].markers,
            vec![DEFAULT_TYPE, POSITIVE_STR, DEFAULT_FEATURE]
        );
    }
}
