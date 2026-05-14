# ADO Service Connection Runbook — UAMI + Workload Identity Federation

Step-by-step recipe for creating an ADO Azure Resource Manager service
connection authenticated by a User-Assigned Managed Identity (UAMI) via
Workload Identity Federation (WIF). This is the SFI-compliant pattern; no
secrets are stored anywhere.

Adapted from Brian's wiki [Creating an ADO Service Connection authenticated
with UMI](https://dev.azure.com/mariner-org/mariner/_wiki/wikis/mariner.wiki/5697/Creating-an-ADO-Service-Connection-authenticated-with-UMI),
with the concrete commands and gotchas from setting up the
`trident-azl4-blob-reader` connection on 2026-05-14.

## What you end up with

```
Azure UAMI ─(federated)→ ADO Service Connection ─(used by)→ Pipeline
   │
   └─(role assignment)→ Target Azure resource
```

The pipeline uses `AzureCLI@2` referencing the SC. ADO mints an OIDC token,
exchanges it for an Azure access token via the UAMI's federated credential,
and the pipeline gets an `az login`'d session with the UAMI's RBAC.

## Prerequisites

- **Azure:** Contributor on the resource group where you'll create the UAMI
- **Azure:** User Access Administrator or Owner on the target resource you're
  granting access to (for the role assignment)
- **ADO:** Project Administrator on the project where the service connection
  will live

## Step 1 — Create the UAMI (Azure CLI)

```powershell
$sub  = "<target-subscription-id>"
$rg   = "<resource-group>"
$loc  = "<region>"        # match siblings if reusing an RG
$umi  = "<umi-name>"      # naming convention: see notes below

az account set -s $sub

# Pre-flight: confirm UAMI doesn't already exist
az identity show -g $rg -n $umi 2>$null
# (should return nothing)

# Create
az identity create -g $rg -n $umi -l $loc `
  --tags purpose=<purpose> owner=<your-alias> project=<project>
```

The output contains `clientId` (use as ADO's Application ID later) and
`principalId` (use as the role-assignment assignee).

### Naming convention notes

Match what's already in the RG. Examples from
`maritimus-github-runner` (b3e01d89... sub):

- `maritimus-github-runner-umi-*` for GitHub Actions identities
- `maritimus-github-storage-ado-*-umi` for ADO pipeline identities

When in doubt, ask the RG owner before deviating.

## Step 2 — Grant the UAMI access to the target resource

For the trident-azl4-blob-reader UAMI, the target was the
`maritimusgithubstorage` storage account, with `Storage Blob Data Reader`
(least privilege — we only need to read base VHDXes).

```powershell
$objId = az identity show -g $rg -n $umi --query principalId -o tsv
$scope = "/subscriptions/$sub/resourceGroups/$rg/providers/<resource-provider>/<resource-type>/<resource-name>"

az role assignment create `
  --assignee-object-id $objId `
  --assignee-principal-type ServicePrincipal `
  --role "<Role Name>" `
  --scope $scope

# Verify
az role assignment list --assignee $objId --all -o table
```

**Always use least privilege.** Don't pick `Owner` when `Reader` will do.

## Step 3 — Start service connection in ADO (do NOT click Verify yet)

In ADO project → Project Settings → Service Connections → New service
connection.

| Field | Value |
|---|---|
| Connection type | **Azure Resource Manager** |
| Identity type | **App registration or managed identity (manual)** |
| Credential | **Workload Identity Federation** |
| Scope Level | **Subscription** |
| Subscription ID | `<sub-id>` |
| Subscription Name | `<sub-name>` |
| **Application (client) ID** | the UAMI's **clientId** from step 1 |
| Tenant ID | `72f988bf-86f1-41af-91ab-2d7cd011db47` (MSIT) |
| Service connection name | `<descriptive-name>` |
| Grant access permission to all pipelines | **uncheck** (see SFI note below) |

After filling these in but **before saving**, ADO shows you:

- **Issuer URL**
- **Subject identifier**

Both are needed for step 4. Keep this ADO tab open.

### Issuer/Subject gotcha — read them off the form

⚠️ Do NOT guess these values. They are not the same as `vstoken.dev.azure.com/...`
that older service connections may show. ADO assigns a new pair when you
create the SC, and the issuer is the Entra tenant authority URL
(`https://login.microsoftonline.com/<tenant>/v2.0`), not the ADO token
issuer URL. The subject is opaque (looks like
`/eid1/c/pub/t/.../sc/.../<sc-guid>`).

Copy the exact strings from the ADO form into the FIC. Do not transcribe;
copy-paste.

## Step 4 — Add the federated credential to the UAMI

```powershell
$issuer  = "<paste from ADO form>"
$subject = "<paste from ADO form>"

az identity federated-credential create `
  -g $rg `
  --identity-name $umi `
  --name "<fic-name>" `
  --issuer  "$issuer" `
  --subject "$subject" `
  --audiences "api://AzureADTokenExchange"

# Verify
az identity federated-credential list -g $rg --identity-name $umi -o table
```

FIC name should describe the consumer. For ADO connections we use
`ado-<project>-<sc-name>` (e.g. `ado-ecf-trident-azl4-blob-reader`).

## Step 5 — Verify and save in ADO

Wait ~30 seconds for Entra to propagate the FIC, then return to the ADO
form and click **Verify and save**.

### Common errors

**`AADSTS70025: client has no configured federated identity credentials`**
- The FIC hasn't been added yet. Run step 4.

**`AADSTS700211: No matching federated identity record found for presented
assertion issuer 'https://login.microsoftonline.com/<tenant>/v2.0'`**
- The FIC exists but the issuer or subject doesn't match what ADO is
  presenting. Re-read the ADO form carefully (do not transcribe — copy).
- A common mistake is reusing the issuer URL from an unrelated existing
  service connection. Each new SC may get its own issuer string.

**Verify succeeds but pipeline fails with `You do not have the required
permissions...`**
- The role assignment in step 2 either targeted the wrong scope, or
  Azure RBAC hasn't propagated yet (wait up to 10 minutes). Re-check that
  `az role assignment list --assignee <principalId> --all` shows the role
  on the correct scope.

## Step 6 — SFI compliance — restrict pipeline permissions

[SFI-ES2.4.11](https://eng.ms/docs/coreai/devdiv/one-engineering-system-1es/1es-docs/1es-security-configuration/azdo-config-remediation/all-pipeline-access-es-2-4-tsg)
prohibits leaving a service connection accessible to all pipelines.

After saving:

1. Open the new service connection in ADO
2. Click **More options (⋮) → Security**
3. Under **Pipeline permissions**, click **Restrict permission**
4. Click **+** and add each pipeline that needs the SC by ID/name. Do not
   add "all pipelines."

## When to use the manual cleanup path

If something goes wrong mid-setup and you need to start over cleanly:

```powershell
# Remove an FIC that pointed at the wrong issuer/subject
az identity federated-credential delete -g $rg --identity-name $umi --name "<fic-name>" --yes

# Confirm no stray role assignments
az role assignment list --assignee <principalId> --all -o table

# In ADO: delete the SC via Project Settings → Service connections → ⋮ → Delete
# In Azure: only delete the UAMI itself if you're sure nothing else uses it
```

The UAMI does no harm by itself — it's a managed identity with role
assignments and FICs. Deleting it cascades to role assignments
automatically; FICs are removed with the parent UAMI.

## Reference — the trident-azl4-blob-reader connection

| Field | Value |
|---|---|
| Purpose | Read AZL4 base VHDX from `maritimusgithubstorage` for trident CI |
| UAMI name | `maritimus-github-storage-ado-trident-reader-umi` |
| UAMI subscription | `b3e01d89-bd55-414f-bbb4-cdfeb2628caa` (`AzureCNMP_CNP_AzureLinux_Polar_ImageTools_Staging`) |
| UAMI resource group | `maritimus-github-runner` |
| UAMI region | `westus2` |
| UAMI clientId | `5eaafbf5-279b-4f16-b797-50bd730dcdb8` |
| UAMI principalId | `97c7c5f1-db58-4e65-8c4a-b6d614a72657` |
| Role granted | `Storage Blob Data Reader` on `maritimusgithubstorage` |
| FIC name | `ado-ecf-trident-azl4-blob-reader` |
| ADO project | `mariner-org/ECF` |
| ADO SC name | `trident-azl4-blob-reader` |
| Pipelines allowed | `[GITHUB]-trident-pr-e2e`, `[GITHUB]-trident-ci`, `[GITHUB]-trident-pr-e2e-azure` |
| Created | 2026-05-14 |

When AZL4 ships in a released MIC container and the
`AzureLinuxArtifacts` ADO feed publishes AZL4 base VHDXes, this whole
connection can be deleted along with the pinned-MIC side-task. Tracking:
`tests/images/mic-azl4-patches/README.md`.
