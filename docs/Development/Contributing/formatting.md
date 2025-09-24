# Formatting

Formatting guidelines in the repository.

Formatting is enforced by Pull Requests checks.

## Overview

You can use this make target to format all code in the project automatically.

```bash
make format
```

## Rust

We adhere to the [Rust style
guide](https://doc.rust-lang.org/nightly/style-guide/). Use cargo to format your
code.

```bash
cargo fmt
```

## Go

We adhere to the style produced by the `gofmt` tool. Use this tool to format all
Go code.

```bash
gofmt -w -s tools/
```

## Python

We adhere to the style produced by [black](https://pypi.org/project/black/) to
format all Python code.

```bash
python3 -m black . --exclude "azure-linux-image-tools"
```

You can use `make format` to format all python files in the project automatically.

You can manually invoke it with `black <file>`, or `black <dir>` to format all
files in a directory.

Recommended version: `23.12` or newer.
