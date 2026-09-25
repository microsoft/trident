# Build a portable static binary

The CI workflow builds the `x86_64-unknown-linux-musl` target natively; the same steps build
`aarch64-unknown-linux-musl` on an arm64 host (swap the target triple below).

Install musl tools and build:

```bash
sudo apt-get update
sudo apt-get install -y musl-tools
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl -p tailor
```

Stage and verify:

```bash
mkdir -p dist
install -Dm755 target/x86_64-unknown-linux-musl/release/tailor \
  dist/tailor-x86_64-unknown-linux-musl
( cd dist && sha256sum tailor-x86_64-unknown-linux-musl > tailor-x86_64-unknown-linux-musl.sha256 )
file dist/tailor-x86_64-unknown-linux-musl
```

The workflow accepts `static-pie linked` or `statically linked`. tailor uses Rustls-based dependencies, so the resulting binary is not dynamically linked to glibc or OpenSSL. Users still need Docker daemon access at runtime.
