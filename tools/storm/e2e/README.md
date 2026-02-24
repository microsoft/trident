# Trident E2E Testing with Storm

- [Trident E2E Testing with Storm](#trident-e2e-testing-with-storm)
  - [Running Locally](#running-locally)
    - [Build](#build)
    - [Query E2E Scenarios](#query-e2e-scenarios)
    - [E2E Scenario Naming](#e2e-scenario-naming)
    - [Run a Specific Scenario](#run-a-specific-scenario)
    - [Help for E2E Scenario](#help-for-e2e-scenario)
  - [How This Works](#how-this-works)
    - [Test Rings](#test-rings)
    - [Discovery](#discovery)
    - [Test Selection](#test-selection)
    - [Validation Test Cases](#validation-test-cases)
    - [Matrix Generation in Pipelines](#matrix-generation-in-pipelines)
    - [Pipeline Execution](#pipeline-execution)
    - [E2E Test Code](#e2e-test-code)

## Running Locally

### Build

```bash
make bin/storm-trident
```

### Query E2E Scenarios

```bash
./bin/storm-trident list scenarios -t e2e
```

You can also filter by other tags for runtime and hardware:

- `host` / `container`
- `vm` / `bm`

### E2E Scenario Naming

All E2E scenarios follow the naming convention:

```
<config>-<hardware>-<runtime>
```

Where:

- `<config>` is the name of the host config used (e.g., `base`, `simple`, `usrverity`).
- `<hardware>` is either `vm` (virtual machine) or `bm` (bare metal).
- `<runtime>` is either `host` (runs directly on the host) or `container` (runs inside a container).

### Run a Specific Scenario

```bash
./bin/storm-trident run <scenario-name> -- <parameters>
```

### Help for E2E Scenario

```bash
./bin/storm-trident run <scenario-name> -- --help
```
<!-- Ugly, update once https://github.com/microsoft/storm/issues/17 is closed. -->

All E2E scenarios have the same underlying code, so parameters are consistent across
scenarios. Common parameters include:

- `-h, --help` Show context-sensitive help.
- `--iso=STRING` Path to the ISO to use for OS installation.
- `--pipeline-run` Indicates whether the scenario is being run in a pipeline
  context. This will, among other things, install dependencies.
- `-i, --test-image-dir="./artifacts/test-image"` Directory containing the test
  images to use for OS installation.
- `--logstream-file="logstream-full.log"` File to write logstream to.
- `--tracestream-file=STRING` File to write tracestream to.
- `--signing-cert=STRING` Path to certificate file to inject into VM EFI
  variables.
- `--dump-ssh-key=STRING` If set, the SSH private key used for VM access will be
  dumped to the specified file.

## How This Works

### Test Rings

This E2E testing framework uses the concept of "test rings" to group scenarios by
their intended frequency of execution.

Inner/lower/fast/earlier rings run more frequently, and outer/higher/slow/later
rings run less frequently. Rings are cumulative, so that all scenarios in inner
rings are also run when an outer ring is run.

The inner-most ring is `pr-e2e`, which is run on every pull request. The
outer-most ring is `full-validation`, which is run weekly.

Test rings are formally defined in
[`testrings/testrings.go`](testrings/testrings.go).

### Discovery

The first step is discovery of all configured E2E scenarios. In short, this
means looking at all existing Host Configurations and when they are supposed to
run. The ultimate goal is to produce instances of the struct `TridentE2EScenario`
(from [`scenario/trident.go`](scenario/trident.go)) representing each
combination of parameters.

All test discovery happens in [`discover.go`](discover.go). The key function is
`DiscoverTridentScenarios`, which returns a list of all discovered
`TridentE2EScenario` instances.

All configurations are defined in `tests/e2e_tests/trident_configurations/`, and
the configuration for when each is supposed to run is defined in
`tests/e2e_tests/target-configurations.yaml`. These are embedded into the Go
binary at compile time using Go's `generate` directive, `embed` package, and
[`invert.py`](invert.py):

```go
//go:generate cp -r ../../../tests/e2e_tests/trident_configurations configurations
//go:generate python3 invert.py
//go:embed configurations/*
var content embed.FS
```

The `invert.py` script reads `target-configurations.yaml` and produces
`configurations/configurations.yaml`, which maps each configuration to its
pipeline ring assignments:

```yaml
<config_name>:
   <hardware_type>:
     <runtime>: <lowest_pipeline_ring>
```

For example:

```yaml
base:
  vm:
    host: pr-e2e
```

The function `DiscoverTridentScenarios` iterates over this data structure and
all the Host Configuration files to produce instances of `TridentE2EScenario`.
Each is configured to contain the Host Configuration, the target hardware type
(`vm`/`bm`), the target runtime (`host`/`container`), and the lowest test ring
it should be run in. If the specific combination is not configured to run in any
ring, the instance is not created.

The type `configs` in [`discover.go`](discover.go) contains the expected
structure of the YAML configuration data.

Some configurations have special behaviors, such as expected failures. For those
special cases, the config can be further customized with the YAML keys defined
in the struct `TridentE2EHostConfigParams` in
[`scenario/trident.go`](scenario/trident.go). These are keys directly under the
configuration name, for example:

```yaml
base:
  maxExpectedFailures: 1 # Expect this config to fail at most once.
  vm:
    host: pr-e2e
```

### Test Selection

Each configuration has a `test-selection.yaml` file
(in `tests/e2e_tests/trident_configurations/<config>/test-selection.yaml`) that
controls which validation test cases run for that configuration. The file is
parsed by [`testselection.go`](testselection.go).

#### Format

```yaml
# Base markers that this configuration supports.
compatible:
  - base
  - encryption

# Optional ring-level overrides. Each ring can add or remove markers
# relative to the compatible set.
weekly:
  add:
    - slow_validation
  remove: []
daily:
  add: []
  remove: []
post_merge:
  add: []
  remove: []
pullrequest:
  add: []
  remove: []
validation:
  add: []
  remove: []
```

The `compatible` list is the base set of test markers. Ring-level overrides
(keyed by ring name: `weekly`, `daily`, `post_merge`, `pullrequest`,
`validation`) can `add` or `remove` markers for specific pipeline stages.

#### Tag Mapping

Each compatible marker is prefixed with `test:` to form a storm scenario tag.
For example, a marker `encryption` becomes the tag `test:encryption`. These tags
determine which validation test cases are registered for the scenario (see
[Validation Test Cases](#validation-test-cases)).

#### All 19 Configurations

| Configuration | Compatible Markers |
|---|---|
| `base` | `base` |
| `simple` | `base` |
| `misc` | `base` |
| `split` | `base` |
| `raid-big` | `base` |
| `raid-mirrored` | `base` |
| `raid-resync-small` | `base` |
| `raid-small` | `base` |
| `combined` | `base`, `usr_verity`, `encryption`, `uki` |
| `encrypted-partition` | `base`, `encryption` |
| `encrypted-raid` | `base`, `encryption` |
| `encrypted-swap` | `base`, `encryption` |
| `extensions` | `base`, `extensions` |
| `health-checks-install` | `rollback` |
| `memory-constraint-combined` | `base`, `usr_verity`, `encryption`, `uki` |
| `rerun` | `base`, `usr_verity`, `encryption`, `uki` |
| `root-verity` | `base`, `root_verity`, `extensions` |
| `usr-verity` | `base`, `usr_verity`, `uki` |
| `usr-verity-raid` | `base`, `usr_verity`, `uki` |

### Validation Test Cases

All E2E validation is implemented in Go under [`scenario/`](scenario/).
Test cases are conditionally registered based on the test tags derived from
`test-selection.yaml`. The registration logic is in
[`scenario/trident.go`](scenario/trident.go) (`RegisterTestCases`).

#### Core Test Cases (always registered)

These run for every scenario:

| Test Case | Description |
|---|---|
| `install-vm-deps` | Install VM dependencies (VM scenarios only) |
| `prepare-hc` | Prepare the host configuration |
| `setup-test-host` | Set up the test host (VM or bare metal) |
| `install-os` | Install the OS via Trident |
| `check-trident-ssh` | Verify Trident via SSH after install |
| `collect-install-boot-metrics` | Collect boot metrics after initial install |
| `publish-logs` | Publish logs and artifacts at scenario end |

#### Tag-Gated Validation Test Cases

These are registered only when the scenario has the corresponding test tag:

| Test Tag | Test Case | Source File | Description |
|---|---|---|---|
| `test:base` | `validate-partitions` | `validate_base.go` | Validate disk partitions match host config |
| `test:base` | `validate-users` | `validate_base.go` | Validate user accounts are created correctly |
| `test:base` | `validate-uefi-fallback` | `validate_base.go` | Validate UEFI fallback boot entry |
| `test:encryption` | `validate-encryption` | `validate_encryption.go` | Validate LUKS2/TPM2 disk encryption |
| `test:root_verity` / `test:usr_verity` | `validate-verity` | `validate_verity.go` | Validate dm-verity configuration |
| `test:extensions` | `validate-extensions` | `validate_extensions.go` | Validate systemd-sysext/confext |
| `test:rollback` | `validate-rollback` | `validate_rollback.go` | Validate health-check rollback behavior |

#### A/B Update Test Cases

For configurations that have A/B update support (`HasABUpdate()`), two sets of
update tests are registered:

**Standard A/B Update** (`ab-update-1-*`):

| Test Case | Description |
|---|---|
| `ab-update-1-sync-hc` | Sync host configuration |
| `ab-update-1-update-hc` | Update host configuration for A/B update |
| `ab-update-1-upload-new-hc` | Upload updated config to test host |
| `ab-update-1-ab-update` | Perform A/B update and reboot |
| `ab-update-1-collect-boot-metrics` | Collect boot metrics after A/B update |

**Split A/B Update** (`ab-update-split-*`, runs on `pre` ring and above):

| Test Case | Description |
|---|---|
| `ab-update-split-sync-hc` | Sync host configuration |
| `ab-update-split-update-hc` | Update host configuration for split update |
| `ab-update-split-upload-new-hc` | Upload updated config to test host |
| `ab-update-split-stage` | Stage the A/B update (without reboot) |
| `ab-update-split-validate-staged` | Validate staging state |
| `ab-update-split-finalize` | Finalize the staged update (reboot) |
| `ab-update-split-collect-boot-metrics` | Collect boot metrics after finalize |

### Matrix Generation in Pipelines

The E2E testing framework includes functionality to generate ADO job matrixes
based on the defined E2E scenarios and their test rings. This allows pipelines to
dynamically adjust which scenarios to run based on the desired test ring.

This is implemented by the `e2e-matrix` script added to `storm-trident` binary,
which is defined in [`matrix_script.go`](matrix_script.go).

This script takes as input the desired test ring (e.g., `pr-e2e`,
`full-validation`) and will output one ADO job matrix per configuration
combination containing all scenarios that should be run at that ring or lower.
The matrices are directly saved as ADO variables that can be consumed by later
jobs in the pipeline, the contents are also printed to standard output for
debugging purposes.

The variable names follow the pattern:

```
TEST_MATRIX_E2E_<HARDWARE>_<RUNTIME>
```

Where `<HARDWARE>` is either `VM` or `BM`, and `<RUNTIME>` is either `HOST` or
`CONTAINER`.

Example:

```text
$ ./bin/storm-trident script e2e-matrix pr-e2e
##vso[task.setvariable variable=TEST_MATRIX_E2E_VM_HOST;isOutput=true]{"base_vm-host":{"scenario":"base_vm-host","hardware":"vm","runtime":"host"}}
INFO[0000] Generated matrix for hardware 'vm' and runtime 'host' with 1 scenarios:
 - base_vm-host 
```

### Pipeline Execution

To run this in pipelines, we depend on two YAML templates:

- [`.pipelines/templates/stages/testing_e2e/storm_e2e.yml`](../../../.pipelines/templates/stages/testing_e2e/storm_e2e.yml)
  This is the entry point for E2E testing in pipelines. It is responsible for
  invoking the matrix generation script and then running the actual test
  execution template for each configuration combination.
- [`.pipelines/templates/stages/testing_e2e/test_execution_template.yml`](../../../.pipelines/templates/stages/testing_e2e/test_execution_template.yml)
  This template is responsible for running the actual E2E scenarios for a given
  configuration combination. It consumes the matrix variable produced by the
  previous template and runs each scenario in it.

#### JUnit XML Output

The pipeline uses Storm's built-in `-j` flag to produce JUnit XML results for
each scenario run. The JUnit XML file is written to the output directory as
`<scenario>_<job-attempt>.junit.xml` and published to ADO via the
`handle-junit-test-results.yml` template. This enables test result visibility
in the ADO test tab for each pipeline run.

### E2E Test Code

All actual E2E test code lives under [`scenario/`](scenario/). The main entry
point is the file [`trident.go`](scenario/trident.go), which contains the
`TridentE2EScenario` struct which implements the storm Scenario interface.

#### Metrics and Log Collection

The scenario automatically collects boot metrics and publishes log artifacts,
eliminating the need for separate YAML pipeline steps:

- **Boot metrics** (`metrics.go`): After the initial OS install and each A/B
  update reboot, the scenario collects `systemd-analyze` boot timing data
  (firmware, loader, kernel, initrd, userspace) via SSH and writes it to
  `boot-metrics.jsonl`.
- **Log publishing** (`logs.go`): At the end of the scenario, all generated log
  and metrics files are published as artifacts via the storm ArtifactBroker:
  - `logstream-full.log` (trident deployment log stream)
  - `trident-clean-install-metrics.jsonl` (netlisten tracestream from install)
  - `boot-metrics.jsonl` (systemd-analyze boot timings)
  - `metrics-*.jsonl` (netlisten tracestream from A/B updates)