# Build work orders

Save a selection as `release.yaml`:

```yaml
schemaVersion: 1
images: [gizmo]
select: {arch: [amd64]}
```

```text
tailor build -w release.yaml --plan
tailor build -w release.yaml --plan --format json
tailor build -w release.yaml
```

The file path is relative to invocation CWD, even with `--manifest` elsewhere.
Order sources conflict with positional images/slugs, `--cell`, `--select` and `--arch`.
Runtime flags (`--output-dir`, `--build-dir-base`, `--force`, `--clones`) stay on the CLI.
See the [schema reference](../reference/work-order.md) for all forms.

`--plan` shows requested/required cells, typed dependencies, serial order and outputs without
engine/registry access, locks, cache writes or signer preflight. It does not predict freshness,
duration or output bytes. It conflicts with the existing IC-invocation `--dry-run`.

## Dependency-safe ADO jobs

```text
tailor matrix --format ado
tailor matrix --ado BUILD_MATRIX --max-jobs 4
```

Job output groups connected dependencies automatically. `--max-jobs` packs whole components
into at most N jobs, largest-first by scheduled-cell count. Components are never split, shared
producers stay together, and cells run serially within each job. JSON/slugs remain cell
inventories; the cap requires ADO output. No generated-file mode or grouping opt-in is needed.

Legs carry scalar JSON `TAILOR_WORK_ORDER`, replacing old per-cell `image`/`slug`/axis variables.
Migrate generation and consumption together. Both jobs need the same checkout revision and Tailor
version; install Tailor and configure suitable pools separately:

```yaml
jobs:
- job: Plan
  steps:
  - checkout: self
  - task: PythonScript@0
    name: emit
    inputs:
      scriptSource: inline
      script: |
        import subprocess
        subprocess.run(
            ["tailor", "matrix", "--ado", "BUILD_MATRIX", "--max-jobs", "4"],
            check=True,
        )

- job: Build
  dependsOn: Plan
  strategy:
    matrix: $[ dependencies.Plan.outputs['emit.BUILD_MATRIX'] ]
    maxParallel: 4
  steps:
  - checkout: self
  - task: PythonScript@0
    env:
      TAILOR_WORK_ORDER: $(TAILOR_WORK_ORDER)
    inputs:
      scriptSource: inline
      script: |
        import subprocess
        command = ["tailor", "build", "--work-order-env", "TAILOR_WORK_ORDER"]
        subprocess.run(command + ["--plan"], check=True)
        subprocess.run(command, check=True)
```

The cap limits generated jobs; ADO `maxParallel` limits simultaneous execution.
Values above are examples, not sizing recommendations. Isolate checkouts and writable paths
between jobs, including RPM farms and logs; different output directories alone are insufficient.

Across stages use `stageDependencies` with explicit dependencies. Python avoids JSON interpolation
and shell tracing of logging commands. Disable xtrace immediately before a logging-command-producing
invocation if using a shell wrapper. Matrix stdout is machine-readable; the roster goes to stderr.
