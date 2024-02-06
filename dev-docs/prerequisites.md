# Prerequisites

- Install [git](https://git-scm.com/downloads). E.g. `sudo apt install git`.
- Install Rust and Cargo: `curl https://sh.rustup.rs -sSf | sh`.
- Install `build-essential`, `pkg-config`, `libssl-dev`, `libclang-dev`, and
  `protobuf-compiler`. E.g. `sudo apt install build-essential pkg-config
  libssl-dev libclang-dev protobuf-compiler`.
- Clone the [Trident
  repository](https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident):
  `git clone https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident`.
- For functional test execution:
  - Clone the [k8s-tests
    repository](https://dev.azure.com/mariner-org/ECF/_git/k8s-tests) and
    [argus-toolkit
    repository](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit) side by
    side with the Trident repository: `git clone
    https://dev.azure.com/mariner-org/ECF/_git/k8s-tests && git clone
    https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit`.
  - Install pytest: `pip install pytest`. Ensure you have at least version 7.0 of
    pytest.
- Change directory to the Trident repository: `cd trident`.
- (Only for changes to `trident_api`) Download documentation dependencies:

  ```bash
  make install-json-schema-for-humans
  ```
