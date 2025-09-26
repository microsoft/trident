# Testing Trident

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
nature of the operations (e.g., manipulating disks, RAID arrays, filesystems,
etc.). For this reason, we have a suite of functional tests that can be run in a
controlled environment. These tests are run as part of our CI/CD pipelines.

Currently, these tests depend on internal resources that are not yet publicly
available.
