# Producer 1266405 read-only image inspection

This temporary branch is a validation-only prerequisite, not a VM E2E result,
image builder, package injector, reviewed packet, or permission probe. Do not
push or dispatch before independent code review and runner admission/access approval.
The temporary workflow admits pushes only to
`user/britel/vm-e2e-image-inspection-20261123`, with the same exact branch guard
on the job. Manual dispatch is retained if admitted by GitHub; a new workflow
cannot rely on off-default-branch dispatch admission. No new grants or credentials
are provisioned. The product revision remains
`18dd6cd99d50a4268afd936acb1ee4d2b64881ea` regardless of harness HEAD.

## Execution boundary

Only an authorized, exclusive disposable Ubuntu 1ES host may invoke the script.
It initializes the normal existing UMI with bounded, suppressed account output.
It neither assumes ECF permission nor uses another principal if access fails.
Missing commands (`az`, `rpm`, `rpm2cpio`, `cpio`, `guestfish`, `zstd`) fail
explicitly before login.
The first actual attempt, run `45272985758`, stopped on missing `guestfish`;
it did not establish ADO content access or image qualification. Its original
failure report is preserved separately, not overwritten by this correction.
The workflow now conditionally installs only the missing `guestfish` binary
package using the already configured Ubuntu apt repositories, with bounded
update/install commands and `--no-install-recommends`. It does not pin the old
version shown in the [official Jammy package metadata](https://packages.ubuntu.com/jammy/guestfish),
add repositories/mirrors, or install the broader `libguestfs-tools` collection.
If the command exists, apt is skipped. Version readiness and all original
inspection dependencies still fail closed.

The whole bootstrap and inspector run under the existing noninteractive sudo
context, including the timeout and owned child-group teardown. Individual
guestfish commands are not wrapped in sudo. No runner UID is assumed: the
workflow captures its numeric UID/GID before elevation. Only executable PATH
and the installed Azure DevOps extension-code directory are carried into the
root context, not credentials or the existing Azure profile. A fresh private
HOME/AZURE_CONFIG_DIR is used before Azure CLI discovery, and the unchanged UMI
login initializes another fresh private inspection profile under that same root
context. Git trusts only this exact checkout through process-local configuration,
not a global safe-directory or ownership change.

Before sudo, the normal job creates a new 0700 publication directory and 0600
`inspection.json` containing an UNREVIEWED failure in the unchanged report shape.
Sudo failure therefore leaves readable bounded evidence and its original exit.
The root inspector pins a writable descriptor before inspection. All privileged
report writes, including failure/cancellation reports, use that descriptor,
never a reopened pathname. Before publication succeeds, the pathname's directory
and file identities must still match both pinned descriptors. A post-validation
file replacement or parent-directory rename/symlink substitution cannot redirect
writes and fails publication with a bounded diagnostic. Only the exact pinned
file and directory are mode/ownership-restored. Symlinks, hard links and mismatched
initial owners are rejected. An inspection failure/cancellation exit and report
are retained even if publication restoration also fails; a publication failure
after successful measurement instead records a failure through the same pinned
descriptor. Workspace ancestors, images, keys and authentication caches are
never chmod/chowned or uploaded.

The existing Azure DevOps CLI extension must already be installed. No automatic
extension installation, host-service setup, Docker or product VM execution occurs.
The inspection appliance uses libguestfs's direct backend without network
enablement. Its own trusted kernel reads filesystems; image payloads never run.
The [official libguestfs FAQ](https://libguestfs.org/guestfs-faq.1.html)
documents the direct backend for root-owned/private image access without
libvirt's separate qemu user; no kernel/private-file permission workaround,
libvirt daemon, new account or group is configured here.
The platform must provide the appliance and its kernel, QEMU, and squashfs/ext4
support. This is inspection infrastructure, not Storm product execution.

The script selects the six preparer artifacts and current-run AZL3 binary RPMs
by fixed immutable artifact IDs. It reuses the preparer's complete ZIP path/type
guards and B transport, redirect handling and unique basename extraction.
Only named tools, both ISOs, both clones of each COSI, and candidate binary RPMs
are extracted. No SSH/private-key artifact or archive member is selected.
Downloads, raw selected logs, RPMDB copies, the private Azure CLI cache, and
inspection scratch are mode-private, outside the single uploaded JSON path.
Cancellation/timeout reuses the reviewed C driver's group teardown: TERM with
bounded grace, then KILL of the whole owned group even if its leader exited first.
The original timeout/cancellation remains the primary failure. Owned
temporary directories are removed on normal failure/cancellation; hard kill or
host loss still requires disposal of the outer 1ES host. No global cleanup runs.

## Measurements and fail-closed layouts

The report includes complete selected file byte lengths and SHA256s, downloaded
archive hashes, source RPM NEVRAs and hashes, installed candidate NEVRAs, critical
payload SHA256s matched to critical files streamed from the exact candidate RPM
via `rpm2cpio` and `cpio --to-stdout` (also checked against its file digests), exact
base package file SHA256, and hashes/counts of fixed original producer task logs.
Source RPMs must use SHA256 file digests. Both target packages `trident` and
`trident-service` are required (the service RPM requires the matching main RPM);
installers require `trident`. Other installed Trident subpackages must also match
a candidate RPM and a known critical payload layout.

Critical paths come from `packaging/rpm/trident.spec`: `/usr/bin/trident`,
`/usr/lib/systemd/system/tridentd.{service,socket}`, the `trident-service` unit,
and applicable subpackage binaries/units/policy/pcrlock files. This is critical
payload identity, not an assertion that every configured file equals its RPM.
No RPM install/scriptlet or image executable is invoked. COSI `osPackages` is
not installed-package proof. Actual RPMDB contents and filesystem payload hashes
are read separately.

COSI selection follows the repository metadata schema and `compare-cosi.py`:
regular tar, `metadata.json`, bounded unique regular members, compressed
`images/*.raw.zst`, compressed SHA384/length and raw length checks, ext4 root and
optional separate `/usr`. Only those two filesystems enter the appliance.
Decompression streams through a bounded pipe, checking the declared length and
absolute 16 GiB ceiling before writing bytes. Overflow stops the producer and its
owned group and removes the incomplete raw file. Truncation fails the exact-length
check; the final raw-file length check remains in place.
Unknown mounts/formats fail. ISO inspection accepts `/liveos/rootfs.img` as a
read-only squashfs filesystem containing the full OS, then reads its RPMDB.
Source research in Azure Linux image-tools `docs/imagecustomizer/concepts/liveos.md`
and `liveosisogrub.go` identifies this bootstrap layout. That source observation
is a layout hypothesis, not proof of the recorded MIC digest's layout. Missing
rootfs, full-OS initramfs, nested filesystem images, non-SQLite RPMDBs, or a live
SQLite WAL fail with a fixed missing-fact diagnostic. They never qualify via a
package-name/configuration proxy. Establish any additional actual layout through
bounded private 1ES inspection and independent review before extending this code.

Base download uses only the recorded Universal Package version
`3.0.20261116` and exact file filter `core-3.0.20261116.vhdx`. ManifestId and
SuperRoot are never treated as file SHA256. The recorded MIC SHA256 is a
previously qualified consumed container digest, not a new tag lookup or file hash.
Each image's original inclusive input citations are retained separately.

## Exact-base download failure diagnostics

Replacement run `45281312918` reported `exact-base-universal-download-failed`
after recording archive/file and candidate RPM measurements. It recorded neither
a base VHDX measurement nor installed image payload proof. Its preserved report
does not contain the child exit code, CLI/extension versions or distinguishing
error evidence. It proves a nonzero CLI exit, not a specific authentication,
permission, ArtifactTool or version defect.

The invocation matches the [official CLI reference](https://learn.microsoft.com/en-us/cli/azure/artifacts/universal?view=azure-cli-latest)
and [project-scoped download/file-filter examples](https://learn.microsoft.com/en-us/azure/devops/artifacts/quickstarts/download-universal-packages?view=azure-devops).
Official extension sources separately perform credential resolution
([services.py](https://github.com/Azure/azure-devops-cli-extension/blob/master/azure-devops/azext_devops/dev/common/services.py))
and ArtifactTool release lookup/bootstrap
([artifacttool_updater.py](https://github.com/Azure/azure-devops-cli-extension/blob/master/azure-devops/azext_devops/dev/common/artifacttool_updater.py)).
Those references are not evidence of the versions installed in the failed run.
Successful ECF byte reads do not prove this separate feed/tool/CLI path works.
No deterministic contract correction or actual access denial is established.

Only a nonzero exit from the exact-base command adds `failure.diagnostic`:
the child exit code, fixed marker labels from at most the first 64 KiB of private
stderr, truncation/read availability, and allowlisted installed CLI/extension
numeric-triplet versions. The local metadata-only `az version --output json`
query is bounded to 30 seconds; its failure/cancellation cannot replace the
original download failure. Unrecognized evidence stays explicitly unclassified
or unavailable. Markers describe observed text, not an independently established
root cause or feed denial. No stderr/stdout fragments, hashes, matched text,
paths, environment, tokens or URLs are emitted. Timeouts/cancellation of the
original command retain their original handling and owned-group teardown.
The download command, identity, immutable selectors and proof gates are unchanged.
There is no retry, CLI/extension upgrade, alternate principal/endpoint, package
or version fallback. A subsequently established access denial remains a blocker.

### Bounded child-local phase observation

Run `45286505815` still reported unclassified, available, untruncated stderr
with CLI `2.91.0` and extension `1.0.2`. This does not establish a cause.
`observe_base_download.py` adds only a local observer for the exact-base call.
It recognizes the exact [2.91.0 Debian launcher](https://github.com/Azure/azure-cli/blob/azure-cli-2.91.0/scripts/release/debian/prepare.sh)
and uses that launcher's installed interpreter with the same isolated `-I`
flag and unchanged `azure.cli` module entrypoint and download arguments.
The launcher's `AZ_INSTALLER=DEB` behavior is retained inside that child only.
No site configuration, installed code, SDK, authentication or selector is patched.

Before observing, complete CLI entrypoint and selected extension module sources
must match fixed SHA256s from CLI `2.91.0` and extension source commit
`1b7f6c0ffdbed8363dd4502cc9acfb25154333be`. CPython 3.10 through 3.14 is supported.
Executing entrypoint/function code objects and fixed function bindings are also
checked against freshly compiled source. An unsupported launcher is invoked
normally without observation. A preflight source/runtime mismatch publishes
only unavailable diagnostics before executing the original launcher once.
A mismatch encountered during execution disables observation without replacing
the CLI result. There is never a second download or alternate strategy.

Run `45475168947` produced a 19453-byte report with SHA256
`b75880cd1b8cc56800e818a40cfd36961fefb84645e31243c9ce5f37a07463d3`.
It reported `source-contract-unavailable`, all phase facts unavailable, and
outer CLI exit 1 with unclassified stderr. That schema did not identify the
rejected gate. Both preflight rejection and runtime rejection discard phase
facts, so the report cannot distinguish them or establish the download cause.
Matching CLI/extension version labels do not establish matching source bytes.

Gate rejection now carries only a fixed `gate.id`, catalogued `gate.reason`,
and `gate.observedSha256`. IDs distinguish the runtime guards, entrypoint
discovery/source/code, extension root/module sources, and each fixed function
contract. A complete observed source SHA256 is emitted only for a hash mismatch
within the unchanged 64 KiB source-read bound. Unreadable, oversized or
unavailable sources have no observed hash; no partial-file hash is reported.
Function/code/binding rejection never exports bytecode, names outside the fixed
catalog, paths, raw source, import errors or exception messages. The unchanged
gate still rejects mismatches, and unavailable phase facts remain unavailable.
This is evidence of an unsupported observer, not evidence of why the original
CLI download returned 1. The original command's separate exit is authoritative.
Historical reports cannot retroactively identify the rejected gate.

Only fixed credential-resolution, release-lookup, tool-bootstrap,
download/extraction, invocation, spawn/wait and SDK-error function boundaries
are observed. Credential-resolution frames contribute boundary events only;
their locals and returned credential objects are not inspected. The diagnostic
records entered/returned boundaries and allowlisted observed exception kinds.
An observed exception may be caught internally, not the final cause.
`returned: true` records an observable normal return. After any exception in
that call, a trace return of `None` cannot reliably distinguish recovery from
exception unwinding, including context-manager or finally cleanup. That return
state remains unavailable (JSON null), even if cleanup executes more lines.
A non-None return or a return without an observed exception is unambiguous.
Repeated calls retain the last return state and last observed exception for
that phase, but exception tracking for return state resets only on a new call.
HTTP status is emitted only as a validated integer from a response's stored
instance fields, associated with its phase. Properties and message parsing
are not used. No fixed typed service-code catalog is established, so service
code remains unavailable.

`releasePresent`, `contentLengthValid` (positive integer), `zipOpened`,
`executablePresent` (the pinned extraction existence branch), `spawnSucceeded`
and `innerExitCode` are recorded only when observable. A cached tool or an
unreached boundary may leave fields unavailable. The inner ArtifactTool exit
is separate from the outer CLI `exitCode`; neither establishes feed access.
JSON null means unavailable, never false or successful qualification.

One validated snapshot, at most 4096 bytes, is written to a private nonblocking
inherited pipe. The child makes its writer non-inheritable before CLI invocation,
and fork descendants cannot publish the observer snapshot. Parent reads never
wait for EOF or late children. Missing, malformed, oversized or failed channels
yield fixed unavailable/invalid labels, never raw data. Timeout/cancellation
remain authoritative; missing final observation is expected when the child is
killed. The existing whole-owned-process-group teardown, private scratch,
pinned writable report FD and publication identity checks are unchanged.
No messages, tracebacks, arguments, headers, URLs, tokens or arbitrary server
strings enter this channel or the report.

Tests exercise isolated native child entrypoint-shaped fixtures, not an
installed Linux CLI or production access. Installed packaging/source matching,
actual runtime cause, base bytes and installed image payload qualification
remain unknown until a separately approved real run.

## Review handoff and remaining gates

`status: complete` means measurements completed, **not** independent approval,
candidate source qualification, or VM success. `complete` retains measured facts;
`failure` preserves the primary sanitized failure; `unavailable` marks missing
qualification. Partial facts are not a successful inspection.
Reports never contain raw log lines, config/history, command output, environment,
headers, SAS URLs, RPM contents or credentials. Only the exact report is uploaded.

The fixed source/download/build log bodies are hashed privately, not asserted to
prove semantic checkout/consumption merely because they exist. An independent
reviewer must privately establish source-built RPM checkout, each image/tool
checkout, current-run RPM download and consumption, and bind these original log
citations to measured output identities. The original task citations must not be
misrepresented as having emitted later installed-package inspection results.
Reference this separately reviewed report for those later facts. No `review`
object or strict preparer packet is fabricated here.

After review, independently create/hash-pin the **unchanged** packet described
in `VM-E2E-Install.md`, run the existing preparer in a separate authorized job,
and only then consider the separately authorized C driver. Actual access,
layout, package equality, base-byte measurement, packet review, preparation, and
product VM execution remain real-runtime gates until their evidence exists.
The separate metadata-only access probe succeeded in run `45265905295`; it
explicitly did not test artifact content access or qualify provenance.

Focused tests use synthetic ZIP/tar/RPMDB/command fixtures only. They are not
producer evidence. Export the exact candidate into a new native Linux `/tmp`
directory before running tests, Black checks and real actionlint. For actionlint
configure only the custom runner label, with no ignored rules.
