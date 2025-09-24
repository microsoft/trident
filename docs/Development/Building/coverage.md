# Code Coverage

Measuring unit test coverage.

This project uses standard code coverage tooling to evaluate code coverage. PR
gates ensure we maintain a high level of code coverage.

## Requirements

1. Install `cargo-llvm-cov` if you haven't already:

   ```bash
   cargo install cargo-llvm-cov --locked
   ```

2. Install `cargo-nextest` if you haven't already:

   ```bash
   cargo install cargo-nextest --locked
   ```

## Running Coverage

```bash
make coverage-llvm
```
