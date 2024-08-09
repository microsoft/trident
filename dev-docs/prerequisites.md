# Prerequisites

- Install [git](https://git-scm.com/downloads). E.g. `sudo apt install git`.
- Install Rust and Cargo: `curl https://sh.rustup.rs -sSf | sh`.
  - The required version of Rust is 1.72.0. To install this version, run `rustup
  install 1.72.0`. To set this as your default version, also run `rustup default
  1.72.0`.
- Install `build-essential`, `pkg-config`, `libssl-dev`, `libclang-dev`, and
  `protobuf-compiler`. E.g. `sudo apt install build-essential pkg-config
  libssl-dev libclang-dev protobuf-compiler`.
- Clone the [Trident
  repository](https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident):
  `git clone https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident`.
- Clone the [argus-toolkit
    repository](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit) side
    by side with the Trident repository: `git clone
    https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit`
- For functional test execution:
  - Clone the [tests
    repository](https://dev.azure.com/mariner-org/ECF/_git/platform-tests) side
    by side with the Trident repository: `git clone
    https://dev.azure.com/mariner-org/ECF/_git/platform-tests`.
  - Install pytest: `pip install pytest`. Ensure you have at least version 7.0
    of pytest.
- Change directory to the Trident repository: `cd trident`.
- (Only for changes to `trident_api`) Download documentation dependencies: `make
  install-json-schema-for-humans`.
- To collect code coverage, install `grcov`: `cargo install grcov`.
