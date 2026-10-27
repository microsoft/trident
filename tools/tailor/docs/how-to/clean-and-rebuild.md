# Clean and rebuild a cell

Find a cell, then remove its ordinary artifact using the janitor (requires a reachable container
engine):

```bash
tailor slugs appliance
tailor clean appliance --cell appliance_amd64_cosi
tailor build appliance --cell appliance_amd64_cosi --force
```

`clean` targets only selected cells' default **uncompressed** artifact names in the workspace's
`artifacts/`, plus the default `<slug>.ca_cert.pem` for signed images using that path. If a
`local-test-ca` profile sets `publishCaCert`, remove that custom path yourself. It does not remove
build stamps, clone outputs, `.zst` compressed outputs, managed extracted `.artifacts` directories,
or a custom `build --output-dir` tree. Remove those separately if needed. `--force` reruns IC even
if a remaining stamp says the cell is current.

A real `build` acquires an advisory lock at `<output-dir>/.tailor/build.lock`. A second build
targeting that output directory fails promptly; use a distinct `--output-dir` for parallel runs.
On interruption, tailor stops the running container and releases the lock. Builds process cells
sequentially; parallel `--jobs` scheduling is not shipped.

See [CLI reference](../reference/cli.md#tailor-clean-images).
