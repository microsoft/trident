package validate

import (
	"fmt"
	"path"
	"strings"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	"github.com/Jeffail/gabs/v2"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/sysinspect"
	tridentutil "tridenttools/storm/utils/trident"
)

// ValidateBase runs the full `base` marker validation (partitions, users, UEFI
// fallback) against the installed host, accumulating all sub-check failures in
// sa. It ports tests/e2e_tests/base_test.py.
//
// expectedState is the servicing state the host is expected to report (normally
// "provisioned"). abActive is the expected active A/B volume; it is only used
// when the host configuration declares an A/B update.
func ValidateBase(
	sa *SoftAsserter,
	client *ssh.Client,
	hs tridentutil.HostStatus,
	expectedState tridentutil.ServicingState,
	abActive tridentutil.AbVolumeSelection,
) {
	spec := hs.Spec()

	ValidatePartitions(sa, client, hs, expectedState, abActive)
	ValidateUsers(sa, client, spec)
	ValidateUefiFallback(sa, client, spec)
}

// ValidatePartitions ports base_test.py::test_partitions. It confirms the
// servicing state, that every configured partition is present both in the Host
// Status and on the running system, and (for A/B configs on non-verity roots)
// that the active volume's device path matches the mounted root device.
func ValidatePartitions(
	sa *SoftAsserter,
	client *ssh.Client,
	hs tridentutil.HostStatus,
	expectedState tridentutil.ServicingState,
	abActive tridentutil.AbVolumeSelection,
) {
	spec := hs.Spec()

	// Gather system state.
	blkid, err := sysinspect.Blkid(client)
	if err != nil {
		sa.Fail("partitions/blkid", err)
		return
	}
	if _, err := sysinspect.Lsblk(client); err != nil {
		sa.Fail("partitions/lsblk", err)
		return
	}

	// Set of PARTLABELs present on the system (partitions_system_info keys in
	// base_test.py).
	presentPartlabels := make(map[string]struct{})
	for _, entry := range blkid {
		if label, ok := entry.Get("PARTLABEL"); ok {
			presentPartlabels[label] = struct{}{}
		}
	}

	// Servicing state.
	sa.Assert("partitions/servicing-state",
		hs.ServicingState() == expectedState,
		"expected servicingState %q, got %q", expectedState, hs.ServicingState())

	// Every configured partition must appear in the Host Status partitionPaths
	// and on the system (as a PARTLABEL).
	partitionPaths := hs.PartitionPaths()
	for _, part := range ExpectedPartitions(spec) {
		if _, ok := partitionPaths[part.ID]; !ok {
			sa.Failf("partitions/status-path",
				"partition %q missing from Host Status partitionPaths", part.ID)
		}
		if _, ok := presentPartlabels[part.ID]; !ok {
			sa.Failf("partitions/system-present",
				"partition %q (PARTLABEL) not found on system", part.ID)
		}
	}

	// A/B active-volume device-path cross-check (non-verity root only; verity
	// A/B is covered by verity validation).
	if spec.HasABUpdate() {
		validateActiveVolumePath(sa, client, hs, spec, blkid, abActive)
	}
}

// validateActiveVolumePath ports the A/B branch of base_test.py::test_partitions
// for non-verity roots.
func validateActiveVolumePath(
	sa *SoftAsserter,
	client *ssh.Client,
	hs tridentutil.HostStatus,
	spec hostconfig.HostConfig,
	blkid map[string]sysinspect.BlkidEntry,
	abActive tridentutil.AbVolumeSelection,
) {
	rootDeviceID, ok := RootFilesystemDeviceID(spec)
	if !ok {
		sa.Failf("partitions/ab-root", "root mount point not found in Host Status spec")
		return
	}

	pairID, rootIsVerity := AbVolumePairID(spec, rootDeviceID)
	if rootIsVerity {
		// Verity root A/B validation lives in the verity validation, not base.
		return
	}

	activeVolumeID, ok := ActiveVolumeID(spec, pairID, abActive)
	if !ok {
		sa.Failf("partitions/ab-active", "no volume pair with id %q for %s", pairID, abActive)
		return
	}

	isPart := IsPartition(spec, activeVolumeID)
	isRaid := IsRaid(spec, activeVolumeID)
	if isPart == isRaid {
		sa.Failf("partitions/ab-kind",
			"active volume %q must be exactly one of partition/raid (partition=%v raid=%v)",
			activeVolumeID, isPart, isRaid)
		return
	}

	// Resolve the device path we expect Host Status to report for the active
	// volume, based on the actual mounted root device.
	rootMountDevice, ok := getRootMountDevice(client)
	if !ok {
		sa.Failf("partitions/ab-mount", "could not determine device mounted at /")
		return
	}
	rootBasename := rootMountDevice[strings.LastIndex(rootMountDevice, "/")+1:]

	var expectedPath string
	switch {
	case isPart:
		entry, ok := blkid[rootBasename]
		if !ok {
			sa.Failf("partitions/ab-blkid", "no blkid entry for root device %q", rootBasename)
			return
		}
		partuuid, ok := entry.Get("PARTUUID")
		if !ok {
			sa.Failf("partitions/ab-partuuid", "root device %q has no PARTUUID", rootBasename)
			return
		}
		expectedPath = "/dev/disk/by-partuuid/" + partuuid
	case isRaid:
		name, found, err := sysinspect.RaidNameForDevice(client, rootMountDevice)
		if err != nil {
			sa.Fail("partitions/ab-raid", err)
			return
		}
		if !found {
			sa.Failf("partitions/ab-raid", "could not resolve RAID name for %q", rootMountDevice)
			return
		}
		expectedPath = name
	}

	if actual, ok := hs.PartitionPaths()[activeVolumeID]; ok {
		sa.Assert("partitions/ab-path-match",
			actual == expectedPath,
			"active volume %q path mismatch: Host Status has %q, expected %q",
			activeVolumeID, actual, expectedPath)
	} else {
		// base_test.py only asserts the path when the active volume ID appears
		// in partitionPaths; when it does not (e.g. combined, where root sits on
		// the A/B pair but Trident reports the mounted device under a different
		// key), it is a no-op rather than a failure. Match that tolerance.
		logrus.Infof("Active volume %q not present in Host Status partitionPaths; skipping path match", activeVolumeID)
	}

	// Active volume selection must match expectation.
	actualVol, present := hs.AbActiveVolume()
	sa.Assert("partitions/ab-active-volume",
		present && actualVol == abActive,
		"expected abActiveVolume %q, got %q (present=%v)", abActive, actualVol, present)
}

// getRootMountDevice returns the device mounted at "/" from `mount` output.
func getRootMountDevice(client *ssh.Client) (string, bool) {
	entries, err := sysinspect.Mount(client)
	if err != nil {
		return "", false
	}
	return sysinspect.RootDevice(entries)
}

// ValidateUsers ports base_test.py::test_users. It confirms that every user and
// group declared in the Host Configuration exists on the system.
// secondaryGroupsField is the Host Configuration schema name for a user's
// secondary groups (trident_api::config::host::os::users::secondary_groups).
//
// It is spelled out here because it was previously read as `groups`, which is
// not a schema field: the lookup silently returned nothing, so group membership
// was never actually validated. The legacy pytest has the same defect
// (base_test.py:427 reads `user_info["groups"]`), so neither suite has ever
// exercised this check.
const secondaryGroupsField = "secondaryGroups"

// SecondaryGroups returns the secondary groups a configured user should belong
// to, skipping any entry that is not a string.
func SecondaryGroups(user *gabs.Container) []string {
	var groups []string
	for _, group := range user.S(secondaryGroupsField).Children() {
		if name, ok := group.Data().(string); ok {
			groups = append(groups, name)
		}
	}
	return groups
}

func ValidateUsers(sa *SoftAsserter, client *ssh.Client, spec hostconfig.HostConfig) {
	systemUsers, err := sysinspect.Users(client)
	if err != nil {
		sa.Fail("users/passwd", err)
		return
	}
	systemGroups, err := sysinspect.Groups(client)
	if err != nil {
		sa.Fail("users/group", err)
		return
	}

	for _, user := range spec.S("os", "users").Children() {
		name, ok := user.S("name").Data().(string)
		if !ok {
			continue
		}
		if _, present := systemUsers[name]; !present {
			sa.Failf("users/present", "configured user %q not found in /etc/passwd", name)
		}

		for _, groupName := range SecondaryGroups(user) {
			members, present := systemGroups[groupName]
			if !present {
				sa.Failf("users/group-present", "configured group %q not found in /etc/group", groupName)
				continue
			}
			if _, ok := members[name]; !ok {
				sa.Failf("users/group-member", "user %q not a member of group %q", name, groupName)
			}
		}
	}
}

// espDeviceId is the device ID the test configurations give the EFI System
// Partition.
const espDeviceId = "esp"

// uefiFallbackDisabled is the mode under which Trident installs no fallback
// boot files at all.
const uefiFallbackDisabled = "disabled"

// EspMountPoint returns the path the EFI System Partition is mounted at,
// according to the Host Configuration.
//
// The mount point is spelled two ways in the schema - a bare path, or an object
// with a `path` - so both are accepted.
func EspMountPoint(spec hostconfig.HostConfig) (string, bool) {
	for _, fs := range spec.S("storage", "filesystems").Children() {
		if id, _ := fs.S("deviceId").Data().(string); id != espDeviceId {
			continue
		}

		mountPoint := fs.S("mountPoint")
		if path, ok := mountPoint.Data().(string); ok {
			return path, path != ""
		}
		if path, ok := mountPoint.S("path").Data().(string); ok {
			return path, path != ""
		}
	}
	return "", false
}

// ValidateUefiFallback checks the UEFI fallback boot files against the
// configured mode.
//
// Only `disabled` is checked here, and deliberately so. The other modes are
// phase-dependent - under `conservative` the fallback points at the servicing
// OS until commit validates the target, so what is correct after a clean
// install differs from what is correct after an A/B update - and this validator
// is handed the Host Configuration and an SSH connection, not the servicing
// operation that just ran. Those modes are covered by the health check the
// scenario injects (uefi-fallback-validation-install / -update), which Trident
// runs per phase on the host and which fails the update itself when the
// fallback is wrong.
//
// `disabled` carries no such ambiguity: "no UEFI fallback boot files are
// installed" holds after an install, an update or a rollback alike.
//
// Ported from base_test.py::test_uefi_fallback, which compared against the
// *current* boot entry unconditionally and read a hard-coded /efi/... path that
// does not exist on these hosts - so the command failed, `&& exit 1 || exit 0`
// turned that into success, and the check passed without testing anything.
func ValidateUefiFallback(sa *SoftAsserter, client *ssh.Client, spec hostconfig.HostConfig) {
	mode := "conservative"
	if m, ok := spec.S("os", "uefiFallback").Data().(string); ok {
		mode = m
	}
	if mode != uefiFallbackDisabled {
		return
	}

	esp, ok := EspMountPoint(spec)
	if !ok {
		// Failing rather than skipping is the point: an unresolvable ESP means
		// the check cannot run, and that must not look like a pass.
		sa.Failf("uefi/disabled", "could not resolve the ESP mount point from the Host Configuration")
		return
	}

	// Probe the mount point itself before the fallback directory, so "the ESP
	// is not where we think it is" is reported as a failure instead of being
	// silently indistinguishable from "the directory is empty".
	// sudo is required throughout: /boot is mode 0700 on these hosts, so the
	// test user cannot even traverse it. Without sudo `test -d` fails with
	// permission denied (reported as a missing ESP) and, worse, `ls` on the
	// fallback directory would fail silently and report zero entries - passing
	// the check no matter what is actually there.
	fallbackDir := path.Join(esp, "EFI", "BOOT")
	cmd := fmt.Sprintf("sudo test -d %q && { sudo ls -A %q 2>/dev/null | wc -l; } || echo MISSING_ESP", esp, fallbackDir)
	out, err := sshutils.RunCommand(client, cmd)
	if err != nil {
		sa.Fail("uefi/disabled", err)
		return
	}

	result := strings.TrimSpace(out.Stdout)
	if result == "MISSING_ESP" {
		// Report what the host actually looks like: an ESP that is not where
		// the Host Configuration says it is means either a real defect or a
		// wrong assumption in this check, and the difference matters.
		sa.Failf("uefi/disabled", "ESP mount point %q does not exist on the host\n%s",
			esp, describeMounts(client, esp))
		return
	}

	// An absent EFI/BOOT counts as no fallback files: `ls` on a missing
	// directory prints nothing, which counts as zero entries.
	sa.Assert("uefi/disabled", result == "0",
		"%s contains %s entries, but uefiFallback is disabled", fallbackDir, result)
}

// describeMounts collects a short picture of the host's mount table and the
// directory the ESP was expected under, for inclusion in a failure message.
func describeMounts(client *ssh.Client, esp string) string {
	var b strings.Builder
	for _, probe := range []struct{ label, cmd string }{
		{"findmnt", "findmnt -n -o TARGET,SOURCE,FSTYPE | grep -iE 'vfat|efi' || echo '(no vfat/efi mounts)'"},
		{"parent", fmt.Sprintf("sudo ls -la %q 2>&1 | head -20", path.Dir(esp))},
		{"esp", fmt.Sprintf("sudo ls -la %q 2>&1 | head -20", esp)},
	} {
		out, err := sshutils.RunCommand(client, probe.cmd)
		if err != nil {
			fmt.Fprintf(&b, "  %s: <%v>\n", probe.label, err)
			continue
		}
		fmt.Fprintf(&b, "  %s:\n%s\n", probe.label, strings.TrimRight(out.Stdout, "\n"))
	}
	return b.String()
}
