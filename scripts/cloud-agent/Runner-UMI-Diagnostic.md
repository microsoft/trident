# Temporary read-only runner UMI diagnostic

This diagnostic observes the existing login boundary, not ADO access or VM E2E.
Run 45723172204 at `fc5e4fa0d2c729fc0a00ac76255e1fdabb0fcaa0` reported
`identity-umi-login`, exit 1. Its private CLI stderr was discarded; that report
does not identify the provider reason. Earlier runs passed the same login.

Parent-verified control-plane evidence confirms the customer hosted pool's
approved UMI assignment. The pool uses shared backing infrastructure in another
subscription/tenant; `SubscriptionNotFound` while reading that subscription
proves neither denial nor absence. This diagnostic does not assume missing
attachment, request backing-resource access or modify any identity.

## Admission and operations

`vm-e2e-umi-diagnostic.yml` admits only the owned branch
`user/britel/vm-e2e-umi-diagnostic-20261125`, including an exact job guard.
Its intended admission is branch push after independent review. Manual dispatch
depends on GitHub workflow registration and has the same guard. It selects
`trident-github-runner-ubuntu2204-amd64`; the existing expensive guest workflow
retains its different exact branch gate and is unchanged.

The workflow uses the same sudo PATH/extension-directory forwarding and existing
protected private-root-profile helper as the execution wrapper. Only a fresh
root HOME/Azure profile is used. There is no comparison against an ambient
runner credential cache and no writes to the existing runner profile.
CLI subprocesses suppress Python bytecode writes to installed code directories;
identity/proxy routing variables are otherwise unchanged.

Exactly three CLI commands are planned:

1. `az version --output json` (30 seconds), retaining only a numeric CLI version.
2. `az login --help` (30 seconds), retaining declaration-presence booleans for
   the four options used by the original invocation.
3. One `az login --identity --client-id
   e2fd61e6-b160-4f02-a5ac-3d949a4fe01d --allow-no-subscriptions --output none`
   (120 seconds), unchanged from the execution wrapper.

There is no fallback selector, second login attempt, explicit IMDS probe,
poll-until-success, ADO token request, image acquisition, guest test, dependency
install, cloud mutation, role change or remote administration. CLI-internal
retry behavior remains bounded by the command deadline. Missing CLI fails
before commands; timeout/cancellation stops later operations. Existing helpers
reap process groups with a 10-second grace. The root context has a five-minute
deadline plus 30 seconds before forced termination.

## Publication and interpretation

Only `runner-umi-report/diagnostic.json`, strictly below 8 KiB, is uploaded as
`umi-login-diagnostic-<run>-<attempt>`. The existing pinned-FD ownership/inode
checks remain authoritative. There is no unsafe publication fallback.

Native stdout/stderr are temporary private files in the new root HOME. At most
64 KiB of each stream is interpreted; oversized/malformed data is unavailable.
No native output, credentials, token/IMDS response, URL, environment value,
command error string or cache is exported. Known identity/proxy environment
variables produce presence booleans only, measured after sudo/private setup.
Unlisted variables are never included.

Failure markers are a closed allowlist of literal CLI/provider observations:
unsupported options, identity/MSI messages, known OAuth/ARM error codes, three
specific Entra codes, and connection/proxy/TLS exception prefixes. Reported HTTP
status must occur in a fixed status-line form and belong to `HTTPStatus`;
conflicting statuses remain null. No status is inferred from an error code or
URL. These observations are not independent proof of root cause or attachment.
Unknown output is explicitly `unclassified`; version/help facts can be null.

The job fails when login fails, times out or is cancelled. Successful login is
labelled only `login-succeeded-not-guest-execution`. Private profile cleanup
uses the existing helper; cleanup/publication uncertainty fails the diagnostic.
Only the report is handed back. Forced termination or host loss can prevent
complete cleanup/publication; every diagnostic host requires disposal.
