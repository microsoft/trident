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
run. To ultimate goal is to produce instances of the struct `TridentE2EScenario`
(from [`scenario/trident.go`](scenario/trident.go)) representing each
combination of parameters.

All test discovery happens in [`discover.go`](discover.go). The key function is
`DiscoverTridentE2EScenarios`, which returns a list of all discovered
`TridentE2EScenario` instances.

[NOTE: IN DEVELOPMENT: Update once this changes.] While in development, all
configurations are defined in `tests/e2e_tests/trident_configurations/`, and the
configuration for when each is supposed to run is defined in
`tests/e2e_tests/target-configurations.yaml`. To port these over into go, we use
a combination of Go's generate directive, Go's embed package, and some custom
code in [`invert.py`](invert.py):

```go
//go:generate cp -r ../../../tests/e2e_tests/trident_configurations configurations
//go:generate python3 invert.py
//go:embed configurations/*
var content embed.FS
```

The python script includes more thorough documentation on itself, but in short
it will produce the yaml file `configurations/configurations.yaml`, which has
this structure:

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

The function `DiscoverTridentE2EScenarios` will go over this data structure and
all the Host Configuration files to produce instances of `TridentE2EScenario`.
Each is configured to contain the Host Configuration, the target hardware type
(`vm`/`bm`), the target runtime (`host`/`container`), and the lowest test ring
it should be run in. If the specific combination is not configured to run in any
ring, the instance is not created.

The type type `configs` in [`discover.go`](discover.go) contains the expected
structure of the YAML configuration data.

Some configuration have special behaviors, such as expected failures. For those
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