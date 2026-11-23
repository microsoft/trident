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
The existing Azure DevOps CLI extension must already be installed. No automatic
extension installation, host-service setup, Docker or product VM execution occurs.
The inspection appliance uses libguestfs's direct backend without network
enablement. Its own trusted kernel reads filesystems; image payloads never run.
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
