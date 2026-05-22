---
sidebar_position: 3
---

# Testing Trident

## Code Checks

To ensure code quality and consistency, run `make check`. This verifies
formatting (`cargo fmt --check`), runs `cargo check` with all features, and
then runs `clippy` with `-D warnings`:

```bash
make check
```

## Unit Testing

To run Trident's unit tests, you can use the following command:

```bash
cargo test --all
```

or

```bash
make test
```

## Functional Testing

Many operations in Trident cannot be tested with unit tests alone given the
nature of the operations (e.g., manipulating disks, RAID arrays, mounts,
filesystems, etc.). For this reason, we have a suite of functional tests that
can be run in a controlled virtual environment. These tests are run as part of
our CI/CD pipelines.

The tests themselves are located in the Rust code under `cfg`
attributes:

```rust
#[cfg(feature = "functional-test")]
mod functional_test {
    // ...
}
```

You can read more about how functional tests work in
[Functional Tests](Functional-Tests.md).

### Prerequisites

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

### Building the Test VM Image

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

### Building Test Dependencies

```bash
# Build virtdeploy (VM management tool)
make bin/virtdeploy

# Build osmodifier
make artifacts/osmodifier

# Build the functional test binaries with code coverage instrumentation
make build-functional-test-cc

# Generate the test manifest (ft.json)
make generate-functional-test-manifest
```

### Running the Tests

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

## E2E Testing

E2E tests validate complete Trident install-and-update workflows using
`netlaunch` to boot a VM from an installer ISO, followed by pytest validation.

See [E2E Tests](E2E-Tests.md) for full setup and run instructions.

## Servicing and Rollback Testing

Servicing and rollback tests use pre-built VM images (defined in
`tests/images/trident-vm-testimage/`) to test multi-update workflows and
manual rollback chains without using `netlaunch` or an installer ISO.

- [Servicing Tests](Servicing-Tests.md) — multi-update loop with optional
  rollback via `storm-trident run servicing`
- [Rollback Tests](Rollback-Tests.md) — full rollback chain (A/B + runtime
  updates) via `storm-trident run rollback`
