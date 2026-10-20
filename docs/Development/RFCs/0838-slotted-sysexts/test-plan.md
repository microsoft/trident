# RFC 0838: Managed Extensions Test Plan

*WIP: TODO: re-review*

Supporting plan for [RFC 0838](../0838-slotted-sysexts.md#testing-and-metrics).
These are planned checks for ACL's `managed`/`acl` implementation and the
`disabled` default, not completed results. `simple` and `native` are deferred.

## Test setup

Use configuration tests for parsing, staging tests for file placement and
interrupted writes, and an ACL VM for actual boot, commit and rollback.
Run boot cases in both directions, A to B and B to A, with SELinux Enforcing.
Cover both bundled COSI images and Host Configuration (HC) URL/OCI inputs.

Give each slot different `/usr` and `/etc` marker contents and create a user
override in writable `/etc`. Before staging, record the running slot's files
and hashes. Check what applications see after boot, not just what exists in
the store.

## 1. Configuration and image sources

| Exercise | Expected result |
| --- | --- |
| Omit the mode or use `disabled`, with bundled images and explicit HC URL/OCI declarations. | Bundled payloads are ignored without unused-image errors; OS servicing and explicit declarations behave as before. |
| Compare `managed` with `acl`; try Boolean, unknown and deferred mode values. | The alias produces the same placement and stored policy. Unsupported inputs are rejected, not treated as another mode. |
| Mix COSI images with HC-declared OCI images, including requested destinations. | Trident fetches and verifies both sources during staging, places them in the target managed store and lists them in its manifest. Boot needs no registry access. |
| Use an ID different from its filename, rename an image, or supply conflicting IDs/names across sources. | Identity and filename are handled separately; conflicts fail before publication rather than silently selecting one source. |
| Corrupt compressed COSI members or HC raw-image hashes; supply malformed or incompatible images. | Each source uses its own hash contract. Invalid inputs never become a published target set. |

## 2. Store replacement and interrupted updates

Start with A committed and populated sets for both slots. Stage B with added,
removed and renamed images; also stage an empty set. A's files must be unchanged
and B must contain only its new set. After a successful update and commit,
repeat with the slots reversed. Fixed-size inputs must not grow storage beyond
the two slot sets.

Attempt cleanup through aliased or escaping paths, unowned directories, and
while A is still needed as the fallback for uncommitted B. The operation must
be refused without changing the protected files. Unrelated runtime activation
entries must be preserved or reported as conflicts.

Interrupt first-use store creation, target clearing, image writes and manifest
publication, including immediately before and after the final rename. Retry
each case. Retained slots must stay bootable; temporary manifests must never
be read, and incomplete sets must never activate.

Switch a previously managed target to `disabled`. Its reused slot gets an
empty manifest and no stale owned images; the retained slot keeps its data.
Native discovery directories and unrelated images are not wiped.

## 3. Manifest parsing and boot integration

Use the [manifest schema](./0838-slot-loader-manifest.schema.json) for structural
fixtures, plus loader tests for duplicate JSON keys, name uniqueness and file
access rules that JSON Schema cannot fully enforce.

| Exercise | Expected result |
| --- | --- |
| Supply only `version`, omit either array, or add unknown root/item fields. | Omitted arrays mean empty. Unknown fields are ignored; known fields are still validated. |
| Remove required fields, use invalid types/versions, duplicate known keys or names, or use unsafe names/symlinks. | The loader rejects the input. Duplicate names still fail when their ignored fields differ. |
| Boot with no store or a valid empty manifest. | Successful exit without links, mounts or an automatic reboot. |
| Keep the store but remove, corrupt or make its manifest unreadable; remove a listed image or invalidate its metadata. | Activation fails, not a fallback to scanning files or an empty set. |
| Trace ROOT mounting, loader startup, Bootengine and the native sysext merge. | The loader runs after ROOT and before `/etc` assembly. Its links survive switch-root; native merge exposes the selected slot's `/usr` content. Loader failure blocks root setup and healthy commit. |

Confirm that the fixed store resolves to shared ROOT; failed reads are errors.
For `/etc`, boot with multiple confexts and overlapping defaults. User changes
win over extension defaults, which win over factory defaults. Updates and
rollback preserve the user upper and select the right shipped defaults.

Missing or empty lowerdir output must preserve factory-only assembly. Exercise
copy-up, whiteouts and changed/removed defaults against the agreed cleanup
policy; finalize those expectations when the RFC's reconciliation TBD is
resolved.

## 4. Update failure and rollback

On a marked, uncommitted update boot, inject a loader error, crash and timeout.
Each must reboot into the committed OS without re-arming the failed entry.
A failure on the fallback's normal boot must not create a reboot loop.

Also exercise bootloader fallback, failed commit and power loss. Confirm the
visible OS and extension markers match after recovery. Fail marker removal
during commit: the previous persistent boot default must remain unchanged.

Read the failure record from the recovered OS and correlate its boot ID and
reason. Repeat with read-only ROOT, failed diagnostic writes and capture
timeouts: lost diagnostics must be reported but must not block reboot.

## 5. Image construction and version compatibility

Boot and reboot base/customized images before Trident datastore initialization,
with absent, empty and populated stores. Image-building edits must keep payloads
and manifests consistent without adopting unrelated factory images.

Stage manifests for same-release and newer target loaders, plus a target that
does not support the chosen format. Preflight must check the target loader,
not the one running on the servicing OS, and reject an unsupported pair before
activation.

Record the retained loader and manifest bytes, update the other slot, then
roll back. Both retained files must be unchanged and boot together; no manifest
conversion or cross-slot reader compatibility is required.

Separately test shared boot/store changes and the retained OS's Trident reader
against the servicing state. These still need
[rollback-safety checks](../0838-slotted-sysexts.md#preserving-rollback-capability).

## 6. Boot-time impact

Follow the RFC's [scenario budgets](../0838-slotted-sysexts.md#metrics), rather
than maintaining a second set of thresholds here. Compare matched boots for
absent-store and empty-manifest no-ops, sysext-only, and confext/mixed workloads.

Report repetition count, image counts/sizes, cold/warm median and p95.
Include service startup and I/O, not just CPU time. For confexts, report
mounting/validation separately from coordination and include the full time
until `/etc` is ready. Existing native sysext merge time stays distinguishable
from newly added loader work.
