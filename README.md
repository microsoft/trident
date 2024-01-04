---
ArtifactType: executable, rpm.
Documentation: ./README.md
Language: rust
Platform: mariner
Stackoverflow: URL
Tags: comma,separated,list,of,tags
---

# Trident

Deployment and update Agent for Mariner OS, allowing for inplace image
deployments and atomic updates. Initial focus is on Bare Metal deployments, but
can be leveraged outside of that as well.

## Contents

- [Trident](#trident)
  - [Contents](#contents)
  - [Docs](#docs)
  - [Getting Started](#getting-started)
  - [Running Trident](#running-trident)
  - [Trident Configuration](#trident-configuration)
  - [Host Configuration](#host-configuration)
    - [Documentation](#documentation)
    - [Schema](#schema)
    - [Sample](#sample)
  - [A/B Update](#ab-update)
    - [Getting Started with Systemd-Sysupdate](#getting-started-with-systemd-sysupdate)
    - [TODO: Next Steps](#todo-next-steps)
  - [gRPC Interface](#grpc-interface)
  - [Running from container](#running-from-container)
  - [Development](#development)
    - [Prerequisites](#prerequisites)
    - [Building and Validating](#building-and-validating)
    - [Reviewing test code coverage](#reviewing-test-code-coverage)
    - [Updating Documentation](#updating-documentation)
    - [Testing hierarchy](#testing-hierarchy)
      - [Unit Tests](#unit-tests)
      - [Functional Tests](#functional-tests)
        - [Functional Test Structure](#functional-test-structure)
        - [Functional Test Environment](#functional-test-environment)
        - [Functional Test Building and Execution](#functional-test-building-and-execution)
        - [Functional Test Code Coverage](#functional-test-code-coverage)
        - [Additional Notes](#additional-notes)
      - [E2E Tests](#e2e-tests)
  - [Contributing](#contributing)
  - [Versioning and changelog](#versioning-and-changelog)
  - [Authors](#authors)
  - [License](#license)
  - [Acknowledgments](#acknowledgments)

## Docs

- [BOM Agnostic Single Node Provisioning
Architecture](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/BOM%20Agnostic%20Single%20Node%20Provisioning%20Architecture.docx?web=1).
- [Trident Agent
  Design](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/Trident%20Agent%20Design.docx?web=1)

## Getting Started

[Deployment
instructions](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?path=/README.md&_a=preview).

## Running Trident

Trident can be automatically started using SystemD (see the [service
definitions](systemd)) or directly started manually. Trident support the
following commands (input as a command line parameter):

- `start-network`: Uses the `network` or `networkOverride` configuration (see below for
  details, loaded from `/etc/trident/config.yaml`) to configure networking in
  the currently running OS. This is mainly use to startup network during initial
  provisioning when default DHCP configuration is not sufficient.
- `run`: Runs Trident in the current OS. This is the main command to use to
  start Trident. Trident will load its configuration from
  `/etc/trident/config.yaml` and start applying the desired HostConfiguration.
  If you in addition pass `--status <path-to-output-file>`, Trident will write
  the resulting Host Status to the specified file.
- `get`: At any point in time, you can request to get the current Host
  Status using this command. This will print the HostStatus to standard output.
  If you in addition pass `--status <path-to-output-file>`, Trident will write
  the Host Status into the specified file instead.

For any of the commands, you can change logging verbosity from the default
`WARN` by passing `--verbosity` and appending one of the following values:
`OFF`, `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`. E.g. `--verbosity DEBUG`.

Note that you can override the configuration path by setting the `--config` parameter.

### Safety check

Trident may destroy user data if run from dev machine or other system that is
not intended to be provisioned. To hopefully avoid this, Trident runs a safety
check before provisioning. The check ensures Linux has been booted from a
ramdisk, and terminates the provisioning process if not. It can be disabled by
creating a file named `override-trident-safety-check` in the root directory.

## Trident Configuration

This configuration file is used by the Trident agent to configure itself. It is
composed of the following sections:

- **allowedOperations**: a combination of flags representing allowed
  operations. This is a list of operations that Trident is allowed to perform on
  the host. Supported flags are:
  - **Update**: Trident will update the host based on the host configuration,
    but it will not transition the host to the new configuration. This is useful
    if you want to drive additional operations on the host outside of Trident.
  - **Transition**: Trident will transition the host to the new configuration,
    which can include rebooting the host. This will only happen if `Update` is
    also specified.

  You can pass multiple flags, separated by `|`. Example: `Update | Transition`.
  You can pass `''` to disable all operations, which would result in getting
  refreshed Host Status, but no operations performed on the host.
- **datastore**: if present, indicates the path to an existing datastore Trident
  should load its state from. This field should not be included when Trident is
  running from the provisioning OS.
- **phonehome**: optional URL to reach out to when networking is up, so Trident
  can report its status. This is useful for debugging and monitoring purposes,
  say by an orchestrator. Note that separately the updates to the Host Status
  can be monitored, once gRPC support is implemented. TODO: document the
  interface, for reference in the meantime
  [src/orchestrate.rs](src/orchestrate.rs).
- **networkOverride**: optional network configuration for the bootstrap OS. If
  not specified, the network configuration from Host Configuration (see below)
  will be used otherwise.
- **grpc**: If present (to make it present, add `listenPort` attribute which
  can be `null` for the default port 50051 or the port number to be used for
  incoming gRPC connections), this indicates that Trident should start a gRPC
  server to listen for commands. The protocol is described by
  [proto/trident.proto](proto/trident.proto). This only applies to the current
  run of Trident. During provisioning, you can control whether gRPC is enabled
  on the runtime OS via the `enableGrpc` field within the Management section of
  the Host Configuration. TODO: implement and document authorization for
  accessing the gRPC endpoint.
- **waitForProvisioningNetwork**: USE WITH CAUTION!! IT WILL INCREASE BOOT
  TIMES IF THE NETWORK CONFIGURATION IS NOT PERFECT. (Only affects clean
  installs) When set to `true`, Trident will start
  `systemd-networkd-wait-online` to wait for the provisioning network to be up
  and configured before starting the provisioning flow. To avoid problems, only
  configure interfaces you know should work and are required for provisioning.
  Try to match by full name to avoid matching interfaces you don't want to. E.g.
  `eth0` instead of `eth*` to avoid matching `eth1` and `eth2` as well.

Additionally, to configure the host, the desired host configuration can be
provided through either one of the following options:

- **hostConfigurationFile**: path to the host configuration file. This is a
  YAML file that describes the host configuration in the Host Configuration
  format. See below details.
- **hostConfiguration**: describes the host configuration. This is the
  configuration that Trident will apply to the host (same payload as
  `hostConfigurationFile`, but directly embedded in the Trident
  configuration). See below details.
- **kickstartFile**: path to the kickstart file. This is a kickstart file that
  describes the host configuration in the kickstart format. WIP, early preview
  only. TODO: document what is supported.
- **kickstart**: describes the host configuration in the kickstart format. This
  is the configuration that Trident will apply to the host (same payload as
  `kickstartFile`, but directly embedded in the Trident configuration). WIP,
  early preview only.

## Host Configuration

Host Configuration describes the desired state of the host.

### Documentation

The full schema is available here:
[trident_api/docs/trident-api.md](trident_api/docs/trident-api.md).

An HTML version is available in `trident_api/docs/html/trident-api.html`

If you have GUI session on Linux, `make view-docs` may help open your browser
automatically. Otherwise you can open the docs
[directly](trident_api/docs/html/trident-api.html). Note that the HTML version
only works when directly opened using your browser, it will not work when opened
using the ADO repo browser or directly in VSCode. We are working on uploading
the docs to the engineering docs site.

You can use the `Live Server` VSCode extension to view the docs in VSCode. After
installing the extension, you can right click on the HTML file and select `Open
with Live Server`. This will open the docs in your default browser.
Alternatively, if you did not want to install the 3rd party extension, you could
just start your own local web server and open the HTML file in your browser,
e.g.: `python3 -m http.server --directory trident_api/docs/html/`.

### Schema

The raw JSON Schema for Host configuration is here: [trident_api/docs/trident-api-schema.json](trident_api/docs/trident-api-schema.json)

### Sample

An example Host Configuration YAML file is available here: [trident_api/docs/sample-host-configuration.yaml](trident_api/docs/sample-host-configuration.yaml)

## A/B Update

Currently, **a basic A/B update flow via systemd-sysupdate** is available with
Trident. The users are able to update the **root** partition and write to
**esp** partition that is part of an A/B volume pair. Other types of partitions
will be eligible for A/B update in a later iteration.

### Getting Started with Systemd-Sysupdate

First, the OS image payload needs to be made available for systemd-sysupdate
to operate on. To use the terms from the sysupdate documentation, the source
image can be published in the following two ways:

1. **regular-file**: The OS image can be bundled with the installer OS and
referenced from the initial HostConfiguration as follows:

   ```yaml
     storage:
       images:
         - url: file:///boot.raw.xz
           sha256:    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
           format: raw-lzma
           targetId: esp
         - url: file:///root.raw.xz
           sha256:    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
           format: raw-lzma
           targetId: root
       abUpdate:
         volumePairs:
           - id: root
             volumeAId: root-a
             volumeBId: root-b
           - id: esp
             volumeAId: esp-a
             volumeBId: esp-b
   ```

   In the sample HostConfiguration above, we're requesting Trident to create
   **two copies of the esp** partition, i.e., a volume pair with id esp that
   contains two partitions esp-a and esp-b, and to place an image in the raw
   lzma format onto esp. First of all, having an esp A/B volume pair is required
   for a successful boot post-update. Second of all, using systemd-sysupdate to
   write to a partition is valid as long as the block device target-id
   corresponds to a partition that is inside of an A/B volume pair. (This is
   because systemd-sysupdate expects 2+ partitions of the given type to do an
   update.) However, the actual A/B update of the esp partition is **not** fully
   supported since the basic e2e flow does not yet implement all the changes
   required to successfully **update the bootloader**. This distinction is very
   important.

2. **url-file**: The OS image can be referenced using remote URLs, at an
HTTP/HTTPS endpoint, e.g. by leveraging Azure blob storage. There are several
requirements per the systemd-sysupdate flow:

   1) Along with the payload, there needs to be **a SHA256SUMS manifest file**
   published in the same remote directory as the image partition files. E.g., if
   the directory contains root_v2.raw.xz, then SHA256SUMS needs to contain the
   following line:

        ```text
        <sha256 hash><2 whitespaces><name of the updated partition file>\n
        ```

   2) The image payload needs to be published with the **.xz extension**, by
   using the LZMA2 compression algorithm, so that systemd-sysupdate can
   decompress the image.

   3) Per current logic, the name of the image partition file corresponds to its
   **version**. Trident will extract the file name from the URL provided by the
   user in the Trident HostConfig and use it inside of the transfer config file,
   to communicate which version is requested from systemd-sysupdate. This means
   that the user needs to use consistent naming for partition files, so that the
   name of the new partition image will be read by systemd-sysupdate as a newer
   version. E.g., a convenient naming scheme could be the following:
   `<partition label/type>_v<version number>.raw.xz` For partition labels, it is
   recommended to use GPT partition type identifiers, as defined in the Type
   section of [systemd repart.d
   manual](https://www.man7.org/linux/man-pages/man5/repart.d.5.html).

   4) The storage.images section in the sample HostConfiguration provided above
   can be set in the following way, to request url-file images for the runtime
   OS:

      ```yaml
      storage:
        images:
          - url: <URL to the boot image>
            sha256: <sha256 hash>
            format: raw-lzma
            targetId: esp
          - url: <URL to the root image>
            sha256: <sha256 hash>
            format: raw-lzma
            targetId: root
      ```

When the installation of the initial runtime OS is completed, the user will be
able to log into the baremetal host, or the VM simulating a BM host. The user
can now request an A/B update by applying an edited Trident HostConfig. To do
so, the user needs to replace the data inside of the storage.images section, to
request to update **root** and write a new image to **esp**, via format
**raw-lzma**, from a new URL, with the sha256 hash taken from SHA256SUMS
published in the first step. For instance, the storage.images section of the new
HostConfig shown above can be changed in the following way:

```yaml
storage:
  images:
    - url: <URL to the updated version of the image>
      sha256: <sha256 hash>
      format: raw-lzma
      targetId: esp
    - url: <URL to the updated version of the image>
      sha256: <sha256 hash>
      format: raw-lzma
      targetId: root
```

- To overwrite the Trident HostConfig, the user can use the following command:

    ```bash
    cat > /etc/trident/config.yaml << EOF
    <body of the updated HostConfig>
    EOF
    ```

    After overwriting the HostConfiguration, the user needs to apply the HostConfig
    by restarting Trident with the following command:

    ```bash
    sudo systemctl restart trident.service
    ```

    The user can view the Trident logs live with the following command:

    ```bash
    sudo journalctl -u trident.service -f
    ```

When the A/B update completes and the baremetal host, or a VM simulating a BM
host, reboots, the user will be able to log back into the host by using the same
credentials. Now, the user can view the changes to the system by displaying the
HostStatus, which is stored in the datastore:
`cat /var/lib/trident/datastore.sqlite`. The user can use commands such as
`blkid` and `mount` to confirm that the partitions have been correctly updated
and that the correct block devices have been mounted at the designated
mountpoints, such as /boot/efi and /.

### TODO: Next Steps

- After A/B update, Trident will be creating an **overlay** file system for the
data/state partitions. This is required so that certain folders, as required by
the user, can be read from and/or written to.
- The user will be able to request an update from a file that is published to
other backends. In the next iteration, Trident will support downloading OS
image payloads published as **OCI artifacts** on Azure Container Registry.
Moreover, based on the users' needs, other image formats might be supported in
the future, beyond raw Zstd and raw Lzma.
- To support downloading OCI artifacts and potentially, other backends,
**a hybrid A/B update** will be implemented: when the user provides a URL link
that systemd-sysupdate cannot correctly download from, Trident will
independently download the payload, decompress it, verify its hash, and point
systemd-sysupdate to the local file, to execute an A/B update. This means that
the overhead associated with generating and publishing the SHA256SUMS manifest
file can be lifted from the user.
- Trident will offer support to update the entire image, i.e. all types of
partitions and not just root, via systemd-sysupdate.
- Encryption and dm-verity will be supported.
- In the next iteration, e2e testing with Trident will be implemented.
Moreover, the next PR will document the performance metrics for the A/B update,
such as the total downtime.
- In the next iteration, Trident will support rollback, in case of an interrupted
or failed A/B update.
- Currently, the basic e2e A/B update flow is only successful when using
kexec() to reboot the system post-update. However, the next iteration will
also support using firmaware reboot, i.e., reboot() in Trident. A mechanism
will be implemented to point the firmware to the correct esp partition; now,
although the GRUB configs are correctly overwritten, the firmware still
attempts to boot into the A partition by default.

## gRPC Interface

If enabled, Trident will start a gRPC server to listen for commands. You can
interact with this server using the [evans gRPC
client](https://github.com/ktr0731/evans). Once installed, you can issue a gRPC
via the following commands:

```bash
# Generate command.json from input/hc.yaml
jq -n --rawfile hc input/hc.yaml '{ hostConfiguration: $hc, allowedOperations: "update | transition" }' > command.json

# Issue gRPC request and pretty print the output as it is streamed back
evans --host <target-ip-adddress> --proto path/to/trident/proto/trident.proto cli call --file command.json UpdateHost | jq -r .status
```

## Running from container

Trident can be run from a container. To build the container, run:

```bash
make docker-build
```

Update `/etc/trident/config.yaml` with the desired configuration.

To run Trident using a docker container, run:

```bash
docker run --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host --pid host trident/trident run
```

## Development

### Prerequisites

- Install [git](https://git-scm.com/downloads). E.g. `sudo apt install git`.
- Install Rust and Cargo: `curl https://sh.rustup.rs -sSf | sh`.
- Install `build-essential`, `pkg-config`, `libssl-dev`, `libclang-dev`, and
  `protobuf-compiler`. E.g. `sudo apt install build-essential pkg-config
  libssl-dev libclang-dev protobuf-compiler`.
- Clone the [Trident
  repository](https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident):
  `git clone https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident`.
- For functional test execution, clone the [k8s-tests
  repository](https://dev.azure.com/mariner-org/ECF/_git/k8s-tests) and
  [argus-toolkit repository](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit) side by side
  with the Trident repository: `git clone
  https://dev.azure.com/mariner-org/ECF/_git/k8s-tests && git clone https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit`.
- Change directory to the Trident repository: `cd trident`.
- (Only for changes to `trident_api`) Download documentation dependencies:

  ```bash
  make install-json-schema-for-humans
  ```

### Building and Validating

Build instructions: `cargo build`.

Run UTs: `make test`. Run UTs with code coverage: `make ut-coverage`.

Collect code coverage report: `make coverage-report`. You can also run `make
coverage' to execute UTs and collect code coverage report. More on that below in
section [Reviewing test code coverage](#reviewing-test-code-coverage).

Run functional tests: `make functional-test`. Rerun tests: `make
patch-functional-test`. More details can be found in the [Functional Tests
section](#functional-tests). If you want to validate the functional test
building, run `make build-functional-test`. `functional-test` and
`patch-functional-test` will automatically ather code coverage data, which can
be viewed using `make coverage-report`.

Validate many steps done by pipelines: `make`.

Rebuild trident_api documentation: `make build-api-docs`.

### Reviewing test code coverage

You can collect the data for computing UT code coverage by running `make
ut-coverage`. This will produce `*.profraw` files under `target/coverage`.

You can collect both the UT and functional test code coverage by running `make
functional-test` or `make patch-functional-test`. This will produce
`*.profraw` files under `target/coverage`.

To view the code coverage report, run `make coverage-report`. This will look for
all `*.profraw` files and produce several coverage reports under
`target/coverage`. It will also print out the overall code coverage from the
available `*.profraw` files. We are currently producing the following reports: `html`,
`lcov`, `covdir`, `cobertura`:

- The `html` report is the easiest to view:
[target/coverage/html/index.html](target/coverage/html/index.html). You can look
at [Documentation](#documentation) section for more details on viewing the
`html` remotely through VSCode.
- The
`lcov` is used by `Coverage Gutters` VSCode extension to show code coverage
directly over the code in the VSCode editor, which helps to see which lines are
covered and which not.
- The `covdir` report is in the JSON format, so
easy for automated processing. The `coverage-report` target actually prints the
overall coverage as extracted from the `covdir` report.
- The `cobertura` report
is something that ADO understands and is published during pipeline run to ADO,
so that we can see code coverage as part of pipeline run results.

### Updating Documentation

After any change to trident_api, the documentation needs to be regenerated. Run:

```bash
make build-api-docs
```

### Testing hierarchy

Developers are expected to accompany any features with unit tests, functional
tests for white box testing, and end to end tests for black box testing. See more
details in the following sections.

#### Unit Tests

Unit tests are meant to test the functionality of a single function in isolation
from everything else. Each module should have a corresponding unit test module
called `tests` annotated with `#[cfg(test)]`. Each test function should be named
`test_*` to indicate which function it is testing and annotated with `#[test]`.

Unit tests can be invoked by `make test` or if code coverage is desired, `make ut-coverage`.

Unit tests should:

- Not be disruptive to the execution environment, so they can
  run on development machines.
- Execute quickly.
- Be deterministic.
- Be independent of each other.
- Not leave any state behind them.
- Not depend on any external resources, that might not be available on the development machine.
- Take advantage of mocking of external resources if possible, to allow testing as much
of the code on the development machine as possible.
- Run in parallel and in a random order.
- Should be the first line of defense for against any regressions.

#### Functional Tests

Functional tests are meant to test the functionality of a module or a set of
functions in a real environment. Each module should have a corresponding functional test module
called `functional_tests` annotated with `#[cfg(all(test, feature =
"functional-tests"))]`. Naming of test function is up to the developer, but
should be indicative of what is being tested. Each test function is annotated with `#[test]`.

Functional tests can be invoked by `make functional-test` and this will
automatically gather code coverage data as well. This `Makefile` target will
automatically deploy a virtual machine and execute the tests inside it. If a
failure is encountered that can be remedied by fixing either the test code or
feature code the test code calls into, you can run `make patch-functional-test`
to update the test binaries and re-run the tests. More details below.

Functional tests should:

- Test the functionality of a module that cannot be easily unit tested in isolation.
- Assume they will not run in parallel to other functions.
- Allow rerunning on the same environment.
- Run as fast as possible.
- Handle validation of external events to the execution environment.
- Aim to target 100% code coverage of the module they are testing (along with
  with the unit tests).

##### Functional Test Structure

Functional tests are structured as follows:

- `/functional_tests`: Contains the functional test code, leveraging
  `pytest` and common SSH interface from `k8s-tests` repo. `pytest` creates the
  test VM using is Fixtures concept and while currently only a single VM is
  created to run all the tests, this could be easily extended to support
  seperate VMs for different tests. Most of the time, no changes will be
  required to this layer while developing functional tests.
- Per module `functional_tests` submodule: Contains the actual test
  implementation written in `rust`, leveraging other code already present in
  Trident. The benefit of this approach is that we can leverage common logic and
  test code is authored side by side with the feature code in a consistent
  manner. This also allows us to easily shift logic between unit and functional
  tests.

Note that additional testing logic can be added as part of
`/functional_tests` as well. At the moment, there are two `pytest` modules
present:

- `test_trident_e2e.py`: Very basic of validation of the main Trident commands:
  `run`, `get` and `start-network`. As you can see in this module, the `pytest`
  logic is used to validate the output of the `get` command using checked in
  `HostStatus`.
- `test_trident_mods.py`: This module invokes the per module functional tests.
  Tests of the following crates are present: `osutils`, `setsail`, `trident` and
  `trident_api`.

The `pytest` logic can be further used to affect the execution environment from
the outside, such as unplugging a disk or rebooting the VM while the tests are
running.

##### Functional Test Environment

The functional test environment is a virtual machine that is deployed by the
`virt-deploy` module. At the moment, only a single VM is created, however the
`pytest` Fixture logic is flexible enough to support multiple VMs when needed.

The VM is created using `virt-deploy` and the initial OS provisioning is done
using `netlaunch` and `Trident`. `Trident` will use the checked in
`functional_tests/trident-setup.yaml` `HostConfiguration` for the initial host
deployment. The tests are started on the deployed OS through SSH connection.

##### Functional Test Building and Execution

There are three ways to build and execute functional tests using `Makefile` targets:

- `make build-functional-test`: This will just build the tests locally and not
  perform any execution. This is useful to ensure the tests are building. The
  output of this step are set of test binaries, one per crate, which is what
  `cargo test` would normally produce and invoke.

- `make functional-test`: This will build the tests locally with code coverage
  profile (using internal `build-functional-test-cc` target), a new `virt-deploy` VM will be created and deployed using
  `netlaunch`. Afterwards, tests will be uploaded into the VM, executed and
  code coverage will be downloaded for later viewing. To note, this will also
  execute all UTs. If you want to iterate on the tests without recreating the
  VM, but do want to redeploy the OS, you can: `make functional-test
  EXTRA_PARAMS="--reuse-environment --redeploy"`.

- `make patch-functional-test`: This will build the tests locally with code
  coverage profile (using internal `build-functional-test-cc` target), upload the
  tests into the existing `virt-deploy` VM, execute the tests and download code
  coverage for later viewing. This is useful when you want to iterate on the
  tests and don't want to wait for the VM to be deployed again. It is important
  to note that only tests that have changed will be re-uploaded. This is
  determined based on `cargo build` output. To note, this will also
  execute all UTs.

To execute the functional tests, ensure that `k8s-tests` and `argus-toolkit` of
recent version are checked out side by side with the `trident` repo.
Additionally, the following dependencies are required for the Ubuntu based
pipelines, so you might need to install them on your development machine as
well:

```bash
sudo apt install -y protobuf-compiler clang-7 bc
sudo apt remove python3-openssl
pip install pytest assertpy paramiko pyopenssl
ssh-keygen -t rsa -b 2048 -f ~/.ssh/id_rsa -q -N ""
```

Paramiko 2.6.0 and 3.x are known to work, .2.6.0 is known to misbehave.

Finally, ensure that you have locally built or populated the provisioning ISO
template and runtime images. You can populate them from the latest pipeline runs
from the `argus-toolkit` repo as follows:

```bash
az login
make download-provision-template-iso
make download-runtime-partition-images
```

If you hit errors and there is no obvious test case failure, it is possible the
error happened during test setup. Unfortunately, the actionable test setup error
log is not at the end, but higher up in the log, much closer to the beginning.
Scroll up until you see a failure by virt-deploy, netlaunch or trident.

##### Functional Test Code Coverage

All functional tests are built with code coverage profile. This means that every
execution will output the profiling data (`*.profraw`), that are needed to
generate the code coverage report. Once the test execution completes, the code
coverage files are downloaded back into the development machine so they can be
aggregated with the locally produced coverage results.

##### Additional Notes

`functional-test` target depends on `k8s-tests` and `argus-toolkit` repos to be
checked out side by side with the `trident` repo. This is because `k8s-tests`
repo contains the common logic to execute test logic over SSH connection and
`argus-toolkit` repo contains the `netlaunch` and `virt-deploy` binaries, along
with logic to generate the OS deployment ISO.

Both `functional-test` and `patch-functional-test` targets leverage `pytest`. To
get more detailed logs or do any changes to the `pytest` logic, you can modify
the command line of the `Makefile` targets (e.g. using `-k`  to select a specific
test case to execute), or you can update `functional_tests/pytest.ini`.

Both `functional-test` and `patch-functional-test` targets leverage
`functional_tests/conftest.py` to setup the initial VM, upload the tests and
download the code coverage. Since the setup VM can be leveraged across multiple
runs of `patch-functional-test`, the VM metadata is stored in a test
directory passed from the `Makefile`: `/tmp/trident-test`. You can inspect the
command line options of `conftest.py` to see what is configurable.

The functional test binaries are produced in a fashion that `cargo test` would
use. That means we can leverage all the feature of `cargo test``, such as
randomizing test order or running tests in parallel without additional custom
code.

#### E2E Tests

End to end tests are meant to test the end to end functionality of Trident in a
real environment. E2E tests are using only public Trident interfaces, by
providing `HostConfiguration` and comparing the status of the host to
`HostStatus`. E2E tests are defined under `/e2e_tests` and are currently only
invoked through the e2e validation pipelines. Each Trident
feature should be accompanied by one or more E2E tests.

End to end tests should:

- Leverage only the user facing interfaces of Trident.
- Assume they will not run in parallel to other functions.
- Handle validation of external events to the execution environment.

## Contributing

Please read our [CONTRIBUTING.md](CONTRIBUTING.md) which outlines all of our
policies, procedures, and requirements for contributing to this project.

## Versioning and changelog

We use [SemVer](http://semver.org/) for versioning. For the versions available,
see the [tags on this repository](link-to-tags-or-other-release-location).

It is a good practice to keep `CHANGELOG.md` file in repository that can be
updated as part of a pull request.

## Authors

[yashpanchal@microsoft.com](mailto:yashpanchal@microsoft.com) - RAID support

## License

This project is licensed under the < INSERT LICENSE NAME > - see the
[LICENSE](LICENSE) file for details

## Acknowledgments

- Hat tip to anyone whose code was used
- Inspiration
- etc
