# Capture Image Customizer logs

Live IC logs stream during a build. To retain a full per-cell debug log:

```bash
tailor --log-dir ./build-logs build appliance
```

Logs are written as `<log-dir>/<slug>.log`; clones use `<slug>_clone<n>.log`. The opt-in order
is `--log-dir` → `TAILOR_LOG_DIR` → `runtime.logDir` (off by default). Relative flag and
environment paths resolve from the current directory; a manifest path resolves from the workspace
root:

```yaml
# tailor.yaml
runtime:
  logDir: build-logs
  logLevel: info
```

IC uses `debug` by default; `--ic-log-level warn` overrides `runtime.logLevel` for a run.
`-v`/`-q` affect tailor's own verbosity, **not** IC's log level.
`--timestamps elapsed|time|off` controls live status/log-line prefixes, not the IC log format.
To disable terminal colors set `NO_COLOR=1`; set `CLICOLOR_FORCE=1` to preserve colors in CI.

See [CLI reference](../reference/cli.md).
