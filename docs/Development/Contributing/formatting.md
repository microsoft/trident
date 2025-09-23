# Formatting

You can use this make target to format all code in the project automatically.

```bash
make format
```

## Rust

Use cargo to format your code.

```bash
cargo fmt
```

## Go

Use `gofmt` to format your code.

```bash
gofmt -w -s tools/
```

## Python

Use [black](https://pypi.org/project/black/) to format your code.

```bash
python3 -m black . --exclude "azure-linux-image-tools"
```

You can use `make format` to format all python files in the project automatically.

You can manually invoke it with `black <file>`, or `black <dir>` to format all
files in a directory.

Recommended version: `23.12` or newer.
