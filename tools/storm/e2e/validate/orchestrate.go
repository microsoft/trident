package validate

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

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
		sa.Failf("partitions/ab-path", "active volume %q missing from partitionPaths", activeVolumeID)
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

		for _, group := range user.S("groups").Children() {
			groupName, ok := group.Data().(string)
			if !ok {
				continue
			}
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

// ValidateUefiFallback ports base_test.py::test_uefi_fallback. It validates the
// UEFI fallback boot entries according to the configured mode (disabled,
// conservative, optimistic; defaulting to conservative).
func ValidateUefiFallback(sa *SoftAsserter, client *ssh.Client, spec hostconfig.HostConfig) {
	mode := "conservative"
	if m, ok := spec.S("os", "uefiFallback").Data().(string); ok {
		mode = m
	}

	switch mode {
	case "disabled":
		// /efi/boot/EFI/BOOT should be empty.
		out, err := sshutils.RunCommand(client, "sudo find /efi/boot/EFI/BOOT/* && exit 1 || exit 0")
		if err != nil {
			sa.Fail("uefi/disabled", err)
			return
		}
		sa.Assert("uefi/disabled", out.Status == 0,
			"/efi/boot/EFI/BOOT is not empty for disabled uefiFallback")
		return
	case "conservative", "optimistic":
		// handled below
	default:
		sa.Failf("uefi/mode", "unknown uefiFallback mode: %q", mode)
		return
	}

	info, err := sysinspect.EfiBootMgr(client)
	if err != nil {
		sa.Fail("uefi/efibootmgr", err)
		return
	}
	currentName, ok := info.CurrentName()
	if !ok {
		sa.Failf("uefi/current", "could not determine current boot entry name (BootCurrent=%q)", info.BootCurrent)
		return
	}

	// Fallback boot files should match the current boot's files.
	cmd := fmt.Sprintf("sudo diff /efi/boot/EFI/BOOT/* /efi/azl/EFI/%s/* && exit 1 || exit 0", currentName)
	out, err := sshutils.RunCommand(client, cmd)
	if err != nil {
		sa.Fail("uefi/diff", err)
		return
	}
	sa.Assert("uefi/diff", out.Status == 0,
		"UEFI fallback files differ from current boot entry %q", currentName)
}
