# justfile — common developer tasks for tailor.
# Run `just` (or `just --list`) to see available recipes. Requires https://github.com/casey/just.
#
# The workspace root is a virtual manifest; the installable binary lives in `crates/tailor`.

bin := "tailor"
pkg := "crates/tailor"

# List the available recipes.
default:
    @just --list

# Build the whole workspace (debug).
build:
    cargo build --workspace

# Build the `tailor` binary in release mode.
release:
    cargo build --release -p {{ bin }}

# Run the CLI, forwarding arguments, e.g. `just run build --dry-run`.
run *args:
    cargo run -p {{ bin }} -- {{ args }}

# Run the full test suite (locked, as in CI).
test:
    cargo test --workspace --locked

# Lint with clippy, warnings as errors (as in CI).
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Format the whole workspace in place.
fmt:
    cargo fmt --all

# Check formatting without writing changes (as in CI).
fmt-check:
    cargo fmt --all --check

# Run the full local gate: format check, lint, tests.
check: fmt-check lint test

# Install the `tailor` binary with cargo (into ~/.cargo/bin).
install:
    cargo install --path {{ pkg }} --locked

# Build a fully static, portable Linux binary (musl). Needs the target + musl-tools:
# rustup target add x86_64-unknown-linux-musl && sudo apt-get install -y musl-tools
static target="x86_64-unknown-linux-musl":
    cargo build --release --target {{ target }} -p {{ bin }}

# Remove build artifacts.
clean:
    cargo clean
