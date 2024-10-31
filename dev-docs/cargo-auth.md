# Using the Private Cargo Registry

*Note: Original instructions can be found in the "Cargo" section
[here](https://dev.azure.com/mariner-org/ECF/_artifacts/feed/BMP_PublicPackages/connect).*

1. Ensure you are using Cargo 1.75 or greater.
2. If you haven't configured credential providers:

   ```bash
   cat << EOF > ~/.cargo/config.toml
   [registry]
   global-credential-providers = ["cargo:token", "cargo:libsecret"]
   EOF
   ```

3. Log in to the Azure CLI if you haven't already:

   ```bash
   az login
   ```

4. Log in to the registry:

   ```bash
   az account get-access-token --query "join(' ', ['Bearer', accessToken])" --output tsv | cargo login --registry BMP_PublicPackages
   ```
