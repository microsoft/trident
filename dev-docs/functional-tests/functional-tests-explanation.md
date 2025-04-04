# Functional Tests Explanation

Functional testing is a pytest test suite (rooted in `functional_tests/`) for
tests that are meant to be run in an isolated environment. Functional testing is
comprised of:

- Native pytest test cases.
- Unit-test like Rust functions.

They are used to test:

- Functionality that cannot work in isolation and depends on external resources
  (e.g. binaries on the system)
- The more dangerous parts of the codebase, generally things you wouldn't want
  to test in your development environment (e.g. operations that require root
  access).

This document aims to explain the architecture of how functional tests work, and
how they are implemented.

- [Functional Tests Explanation](#functional-tests-explanation)
  - [Native Python Tests](#native-python-tests)
  - [Rust-Based Tests](#rust-based-tests)
    - [Conditional Compilation](#conditional-compilation)
    - [Test Case Definition](#test-case-definition)
    - [Inventory Collection](#inventory-collection)
    - [Rust-Pytest Interface](#rust-pytest-interface)
      - [Rust Export to JSON](#rust-export-to-json)
      - [Pytest Collection from JSON](#pytest-collection-from-json)
  - [Isolated Environment Creation](#isolated-environment-creation)

## Native Python Tests

Native Python tests are pytest tests that are written in Python. They live in
python files in the `functional_tests/custom` directory.

Generally, these leverage the `vm` fixture, which is a Mariner VM that has been
created using `virt-deploy` and `netlaunch`. The `vm` fixture is defined in
`conftest.py`.

## Rust-Based Tests

### Conditional Compilation

Rust-based functional tests should be contained within a module called `functional_test`
gated by the feature `functional-test`. For example:

```rust
#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    // tests here
}
```

### Test Case Definition

Each test case should have the proc-macro attribute `#[functional_test]` applied
to it. The attribute is defined in `pytest_gen`.

This attribute expands roughly to the following:

```rust
inventory::submit!{pytest::TestCaseMetadata {
    module: module_path!(),
    function: #function,
    negative: #negative,
    feature: #feature,
    type_: #test_type,
}}

#[test]
#(#attrs)*
#vis #sig {
    #block
}
```

For example, the following test case:

```rust
#[functional_test]
fn test_case() {
    // test here
}
```

Expands to:

```rust
inventory::submit!{pytest::TestCaseMetadata {
    module: module_path!(), // This will be `crate::module1::moduleN::functional_test`
    function: "test_case",
    negative: false,
    feature: "",
    type_: "",
}}

#[test]
fn test_case() {
    // test here
}
```

The key part is the `inventory::submit!` macro. This macro comes from the
`inventory` crate, which is generally used to register plug-ins. In this case,
the object being registered is a `TestCaseMetadata` object, which contains all
the information needed to run the test case.

The `inventory` crate creates a global list of all the registered objects, which
can be consumed lated. This happens in the `pytest` crate.

### Inventory Collection

The `pytest` crate contains the definition of the `TestCaseMetadata` object, and
thus, also contains the macro `inventory::collect!` which is used to collect all
the registered `TestCaseMetadata` objects.

The `pytest` crate is responsible for iterating over all the submitted test
cases. This happens in the `generate_functional_test_manifest()` function, which
is called by Trident's `pytest` subcommand. This subcommand is ONLY available
when the `pytest-generator` cargo feature is enabled.

### Rust-Pytest Interface

To run the functional tests, pytest must be aware of all the test cases that
exist within the test binaries.

#### Rust Export to JSON

The `pytest` crate collects all the test cases, and builds a tree representing
rust's modules, and all the test cases within them. This tree is then serialized
into a JSON file called `ft.json`, and placed in the `functional_tests/`
directory.

The underlying structure of the JSON file is:

```rust
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
}

let mut json_output: HashMap<String, Module> = HashMap::new();
```

An example of the JSON output is:

```json
{
  "osutils": {
    "submodules": {
      "sfdisk": {
        "submodules": {
          "functional_test": {
            "test_cases": {
              "test_get": {
                "markers": [
                  "functional",
                  "positive",
                  "helpers"
                ]
              }
            }
          }
        }
      },
      "container": {
        "submodules": {
          "functional_test": {
            "test_cases": {
              "test_get_host_root_path_in_simulated_container": {
                "markers": [
                  "functional",
                  "positive",
                  "helpers"
                ]
              },
              "test_get_host_root_path_fails_in_simulated_container_without_host_mount": {
                "markers": [
                  "functional",
                  "negative",
                  "helpers"
                ]
              },
            }
          }
        }
      }
    }
  }
}
```

#### Pytest Collection from JSON

In pytest, we use the hook `pytest_collect_file` to look out for the `ft.json`
file. When we find it, we parse it, and create a `FuncTestCollector` object,
which returns one `RustModule` object per crate.

The `RustModule` object explores the tree and recursively yields more
`RustModule` objects, until it reaches the test cases, which it yields as
`pytest.Function` objects.

The `pytest.Function` objects are partials created from the function

```python
def run_rust_functional_test(vm, crate, module_path, test_case):
    """Runs a rust test on the VM."""
    from functional_tests.tools.runner import RunnerTool

    testRunner = RunnerTool(vm)
    testRunner.run(
        crate,
        f"{module_path}::{test_case}",
    )
```

All the parameters except for `vm` get filled in during collection.

Pytest will then run the `run_rust_functional_test` function, which will run the
test case inside of the VM.

## Isolated Environment Creation

The functional test pytest suite defines the `vm` fixture, whose setup includes
calls to `virt-deploy` and netlaunch to create a Mariner VM ready to receive SSH
connections.

The OS is provisioned using `functional_tests/trident-setup.yaml`.
