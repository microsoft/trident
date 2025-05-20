# Prerequisites

- Install [git](https://git-scm.com/downloads). E.g. `sudo apt install git`.
- Set up git credential manager (GCM) for Azure DevOps. Follow the instructions:
  - Ubuntu:
    - Uninstall any and ALL dotnet runtimes and SDKs: `sudo apt remove --purge
      dotnet*`.
    - Set up the package sources as necessary depending on your distro/version
      (Not needed for Ubuntu 22.04, the package is available from Ubuntu's repo).
    - Install `dotnet-runtime-7.0` and `dotnet-runtime-7.0`.
  - AzL:
    - [Accessing ADO Repos with Git Credential
      Manager](https://dev.azure.com/mariner-org/mariner/_wiki/wikis/mariner.wiki/4263/Accessing-ADO-Repos-with-Git-Credential-Manager)
  - Set up GCM:
    ([Instructions](https://eng.ms/docs/cloud-ai-platform/devdiv/one-engineering-system-1es/1es-docs/1es-security-configuration/configuration-guides/gcm?tabs=linux-install))
    - Summary:

        ```bash
        dotnet tool install -g git-credential-manager 
        git-credential-manager configure 
        git config --global credential.azreposCredentialType oauth
        ```

- Install Rust and Cargo: `curl https://sh.rustup.rs -sSf | sh`.
  - The required version of Rust is 1.72.0. To install this version, run `rustup
  install 1.72.0`. To set this as your default version, also run `rustup default
  1.72.0`.
- Install `build-essential`, `pkg-config`, `libssl-dev`, `libclang-dev`, and
  `protobuf-compiler`. E.g. `sudo apt install build-essential pkg-config
  libssl-dev libclang-dev protobuf-compiler`.
- Install the `virt-firmware` Python package: `sudo pip3 install virt-firmware`.
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
- To collect code coverage, install `grcov`: `cargo install grcov`.
