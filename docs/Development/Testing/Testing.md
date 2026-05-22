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

Functional tests validate operations that cannot run in isolation (disk
manipulation, RAID arrays, mounts, filesystems, etc.) inside a libvirt/QEMU
virtual machine.

See [Functional Tests](Functional-Tests.md) for architecture details,
prerequisites, building the test VM image, and running the tests.

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

## Code Coverage

See [Coverage](Coverage.md) for instructions on generating and viewing code
coverage reports.
