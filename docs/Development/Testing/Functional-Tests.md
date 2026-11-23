---
sidebar_position: 5
---

# Functional Tests

Functional testing is a `pytest` test suite (rooted in `tests/functional_tests/`) for
tests that are meant to be run in an isolated environment. Functional testing is
comprised of:

- Native pytest test cases.
- Unit-test like Rust functions.

They are used to test:

- Functionality that cannot work in isolation and depends on external resources
  (e.g. binaries on the system)
- The more dangerous parts of the codebase, generally things you wouldn't want
  to test in your development environment such as:
  - Operations that require root access.
  - Operations that manipulate disks, RAID arrays, mounts, filesystems, etc.
  - Operations that modify the OS in any way.

## Prerequisites

Functional tests run inside a libvirt/QEMU virtual machine. You need:

- **Linux host** with root access (functional tests manipulate disks, mounts,
  etc.)
- **libvirt and QEMU** installed and configured
- **Docker** (to run Image Customizer for building the test VM image)
- **[oras](https://oras.land/)** CLI (to download base images from MCR)
- **Go 1.24+** (to build `virtdeploy`)
- **Python 3.8+** with test packages:

  ```bash
  pip3 install -r tests/functional_tests/requirements.txt
  ```

## Building the Test VM Image

The functional test VM image is an Azure Linux 3 QCOW2 image built with
[Image Customizer](https://github.com/microsoft/azure-linux-image-tools). The
build uses a container from MCR (`mcr.microsoft.com/azurelinux/imagecustomizer:latest`)
and a base image also from MCR.

1. **Download the base image:**

   ```bash
   # Downloads baremetal.vhdx from mcr.microsoft.com/azurelinux/3.0/image/baremetal:latest
   ./tests/images/testimages.py download-image baremetal
   ```

2. **Build the functional test image:**

   ```bash
   sudo ./tests/images/testimages.py build trident-functest --output-dir ./artifacts
   ```

   This produces `artifacts/trident-functest.qcow2`. The image configuration is
   defined in `tests/images/trident-functest/base/baseimg.yaml`.

## Building Test Dependencies

These dependencies are built automatically by `make functional-test`, but you
can build them individually if needed:

```bash
# Build virtdeploy (VM management tool)
make bin/virtdeploy

# Build the functional test binaries with code coverage instrumentation
make build-functional-test-cc

# Generate the test manifest (ft.json)
make generate-functional-test-manifest
```

## Running the Tests

Run the full functional test suite:

```bash
make functional-test
```

This will create a VM using `virtdeploy`, upload the test binaries, and run all
tests via pytest.

To rerun tests on an already-running VM (faster iteration):

```bash
make patch-functional-test
```

To run a subset of tests, use the `FILTER` variable:

```bash
make functional-test FILTER="custom/test_trident_e2e.py -k test_name"
```

### Selecting the Functional VM

The functional pytest suite accepts these optional arguments:

| Argument | Default for a new environment |
|----------|-------------------------------|
| `--virtdeploy-namespace` | `virtdeploy` |
| `--virtdeploy-network` | `192.168.242.0/24` (virtdeploy's default) |
| `--functional-base-image` | `artifacts/trident-functest.qcow2` in the repository |

An explicit base-image path expands `~` and resolves relative to the caller's
working directory. The image must already exist. For example:

```bash
make functional-test \
  FUNCTIONAL_TEST_EXTRA_PARAMS="--virtdeploy-namespace functional-run --virtdeploy-network 192.168.243.0/24 --functional-base-image /path/to/trident-functest.qcow2"
```

These arguments select the VM created by the functional fixtures, not other E2E
runners. They do not bypass the Make target's normal build/image prerequisites.

### Reuse and Cleanup

Creation caches the VM address, namespace, network and resolved base-image path
after the VM comes online. `--reuse-environment` skips creation and restores the
cached namespace. Each explicitly supplied selection argument must match its
cached value (network strings are compared exactly); missing or mismatched
metadata fails with a request to recreate the environment.

Legacy caches without namespace metadata remain usable when none of the three
selection arguments is supplied. This logs a warning and uses the default
`virtdeploy` namespace. Explicit selection requires verifiable cached metadata,
including the namespace.

To reuse an environment, retain the pytest cache and pass `--test-dir` pointing
to its existing test directory, including `known_hosts`. `make functional-test`
and `make patch-functional-test` use `/tmp/trident-test` by default and both pass
`--keep-environment`; the patch target also passes `--reuse-environment`.

The `vm` fixture downloads coverage, then, unless `--keep-environment` is set,
runs `bin/virtdeploy clean --namespace` with the created or restored namespace.
Cached VM metadata is cleared only after successful cleanup; cleanup failures
propagate and retain the cache. With `--keep-environment`, coverage is still
downloaded, but the VM and cache are retained. The caller or enclosing runner
owns subsequent cleanup.

### GitHub cloud agent

The cloud runner requires an **exclusive, disposable outer 1ES test runner**
with nested virtualization and the data disk mounted at `/mnt/storage`. Multiple
nested VMs are legitimate, but unrelated workloads and shared-host dependency
safety are outside this contract. Do not use these scripts on a shared developer
host. Source setup and `scripts/cloud-agent/validate.sh` default to source-only
validation; functional execution is always explicit.

Install the optional libvirt/QEMU dependencies once:

```bash
bash scripts/cloud-agent/setup-functional.sh
```

Setup installs `xorriso` for cloud-init ISO generation and preserves existing
libvirt configuration and SSH keys. If an existing private key lacks its public
key, setup derives it without changing the private key. This requires a readable,
unencrypted private key; otherwise setup fails noninteractively. Runs explicitly
select `qemu:///system` without rewriting the user's libvirt configuration.

Run the bounded functional smoke suite:

```bash
scripts/cloud-agent/validate.sh functional
```

The downloader resolves the latest successful completed `trident-ci` build from
`main`. It records the immutable build ID, source commit, artifact metadata,
image size and SHA-256 in `provenance.json` beside the cached image. The smoke
suite creates the VM, proves SSH connectivity and runs positive `helpers`
functional tests. To run the complete functional suite instead:

```bash
bash scripts/cloud-agent/run-functional.sh full
```

Both `smoke` and `full` run **functional tests only**, not all E2E suites.
Record the mode, result, build ID, source commit and image SHA-256 as validation
evidence. To select the same build again:

```bash
TRIDENT_FUNCTIONAL_BUILD_ID=<build-id> scripts/cloud-agent/validate.sh functional
```

Images are cached by immutable build ID under
`/mnt/storage/trident-cloud-agent/functional-$(id -u)`. Setup and the downloader
use per-UID caches so accounts do not reuse another account's images or cache
locks. The old shared `functional` cache is left untouched, not migrated.
`TRIDENT_FUNCTIONAL_CACHE_DIR` or the downloader's `--output-root` can override
the default; overrides must be writable by the current account and should not be
shared across UIDs. Cache preparation is locked per build, and interrupted
image/provenance publication is recovered on retry. The immutable image path is
passed directly to pytest; an existing `artifacts/trident-functest.qcow2` is left
untouched. Publishing a compatibility symlink requires the downloader's explicit
`--link-path`.

Runs hold `/var/lib/trident-cloud-agent/functional.lock`, provisioned for the
`libvirt` group, from artifact preparation and shared build generation through
namespace cleanup. Competing runs exit with a host-busy error. This lock only
coordinates participating updated scripts, not independent libvirt tools or
older runners.

Cloud allocation conservatively rejects existing or prefix-colliding libvirt
resource names and nonempty or uninspectable namespace storage directories.
It selects a subnet that does not overlap host IPv4 routes or defined libvirt
networks; `TRIDENT_FUNCTIONAL_NETWORK` may request a noncolliding subnet.
`TRIDENT_FUNCTIONAL_NAMESPACE` must start with a letter or digit and contain
only letters, digits, underscores or hyphens. These are allocation checks, not
proof that other VMs do not reference the same files.

The runner passes `--virtdeploy-namespace`, `--virtdeploy-network` and
`--functional-base-image` to the shared fixtures. Its pytest cache and the
fixture's netlaunch configuration are private temporary files, leaving the
checkout's existing cache and `tools/vm-netlaunch.yaml` untouched.
`--keep-environment` defers VM cleanup to the runner's EXIT trap, which cleans
the matching namespace on success or failure.

Virtdeploy uses type-aware namespace selection and narrow canonical pool target
validation. It does not scan arbitrary cross-namespace filesystem dependencies,
migrate ownership metadata or provide an unsafe bypass. Bind mounts, backing
chains, QEMU passthrough files and TFTP roots are not universally protected by
these checks. The exclusive disposable runner contract remains required.

## Architecture

This section explains the architecture of how functional tests work and
how they are implemented.

## Native Python Tests

Native Python tests are pytest tests that are written in Python. They live in
python files in the `tests/functional_tests/custom` directory.

Generally, these leverage the `vm` fixture, which is an Azure Linux VM that has been
created using `virt-deploy`. The `vm` fixture is defined in `conftest.py`.

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
    xfail: #xfail,
    skip: #skip,
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
    xfail: None,
    skip: None,
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
into a JSON file called `ft.json`, and placed in the `tests/functional_tests/`
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

The functional test pytest suite defines the `vm` fixture. This VM is created
using `virt-deploy` with a prebuilt Azure Linux 3 image.
