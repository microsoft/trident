---
sidebar_position: 3
---

# Testing Trident

## Code Checks

To ensure code quality and consistency, we use Rust's `clippy` linting tool. You
can run it with the following command:

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

:::info NOTICE
Running these tests depends on internal resources that are not publicly
available yet. 
:::

Many operations in Trident cannot be tested with unit tests alone given the
nature of the operations (e.g., manipulating disks, RAID arrays, mounts,
filesystems, etc.). For this reason, we have a suite of functional tests that
can be run in a controlled virtual environment. These tests are run as part of
our CI/CD pipelines.

The tests themselves are located in the Rust code under `cfg`
attributes:

```rust
#[cfg(feature = "functional-tests")]
mod functional_tests {
    // ...
}
```

You can read more about how functional tests work in
[Functional Tests](functional-tests.md).
