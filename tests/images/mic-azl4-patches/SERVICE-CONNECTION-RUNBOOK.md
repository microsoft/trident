# ADO Service Connection runbook — AZL4 blob reader

The trident CI pipeline's new AZL4 build stage (`build-image-azl4.yml`) reads
the AZL4 base VHDX from Azure Blob Storage. It authenticates via an Azure
service connection in the ADO project. **This connection must be created
manually** before the pipeline can succeed; Karhu can't do this step
unattended.

## What to create

- **Service connection type:** Azure Resource Manager → Workload Identity
  Federation (preferred) or Service Principal (Manual)
- **Service connection name:** `trident-azl4-blob-reader`
  - Must match the `blobServiceConnection` parameter default in
    `.pipelines/templates/stages/build_image/build-image-template-azl4.yml`
  - If a different name is required by project policy, update the YAML
    parameter at the same time
- **Scope:** Subscription containing `maritimusgithubstorage`
  - As of 2026-05-12 that storage account is in the same subscription
    Vince's `microsoft/azurelinux-image-tools` workflow uses
- **Resource scope to grant access to:** the storage account
  `maritimusgithubstorage` (NOT the entire subscription)

## Required Azure RBAC role

On the storage account `maritimusgithubstorage`:

- **Role:** `Storage Blob Data Reader`
- **Assignee:** the service principal / managed identity behind the new
  service connection
- **Scope:** resource (the storage account itself)

`Reader` (control plane) is NOT enough — we need the data plane role.

## Step-by-step (ADO portal)

1. ADO Project Settings → Service connections → New service connection
2. Pick "Azure Resource Manager"
3. Pick "Workload Identity federation (manual)" — recommended; avoids
   long-lived secrets
4. Name: `trident-azl4-blob-reader`
5. Subscription: the one hosting `maritimusgithubstorage`
6. Grant access permission to all pipelines: yes (or restrict to specific
   pipelines if project policy requires)
7. After creation, copy the federated identity's app/object ID
8. In the Azure portal → `maritimusgithubstorage` → Access control (IAM)
   → Add role assignment → `Storage Blob Data Reader` → pick the service
   principal from step 7

## Verification

After creating, run the pipeline manually. The "Download AZL4 base VHDX from
blob" step should succeed and log a line like:

```
INFO:builder.download:Latest: azure-linux/core-efi-vhdx-4.0-amd64/4.0.YYYYMMDD/image.vhdx
```

If it fails with "You do not have the required permissions needed to perform
this operation. Depending on your operation, you may need to be assigned one
of the following roles: 'Storage Blob Data Reader' ..." the RBAC grant in
step 8 did not propagate. Re-check the assignment scope (must be the storage
account itself, not the subscription) and wait ~5 minutes for caches.

## Who can do this

- ADO Project Admin can create the service connection
- Owner / User Access Administrator on the subscription containing
  `maritimusgithubstorage` can grant the RBAC role

## When to delete

Drop this whole flow when AZL4 base VHDXes are published to the
`AzureLinuxArtifacts` ADO feed and we switch back to the standard
`base-images-download-template.yaml@platform-pipelines` path. Tracking:
`tests/images/mic-azl4-patches/README.md`.
