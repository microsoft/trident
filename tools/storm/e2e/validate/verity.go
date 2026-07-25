package validate

import (
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/sysinspect"
	tridentutil "tridenttools/storm/utils/trident"
)

// VerityDevice describes a verity device declared in the Host Status spec.
type VerityDevice struct {
	ID           string
	Name         string
	DataDeviceID string
	HashDeviceID string
}

// HasVerity reports whether the spec declares any verity device, used to
// self-select verity validation.
func HasVerity(hs tridentutil.HostStatus) bool {
	return hs.Spec().Exists("storage", "verity")
}

// verityForRoot returns the verity device whose id matches the root
// filesystem's device ID, and whether it was found.
func verityForRoot(spec hostconfig.HostConfig, rootDeviceID string) (VerityDevice, bool) {
	for _, v := range spec.S("storage", "verity").Children() {
		id, _ := v.S("id").Data().(string)
		if id != rootDeviceID {
			continue
		}
		dev := VerityDevice{ID: id}
		dev.Name, _ = v.S("name").Data().(string)
		dev.DataDeviceID, _ = v.S("dataDeviceId").Data().(string)
		dev.HashDeviceID, _ = v.S("hashDeviceId").Data().(string)
		return dev, true
	}
	return VerityDevice{}, false
}

// ValidateVerity ports verity_test.py::test_verity_root. It confirms the root
// verity device mapper exists and is active/verified/read-only, then validates
// that its data and hash devices correspond to the expected block devices
// (accounting for A/B updates and RAID arrays).
func ValidateVerity(
	sa *SoftAsserter,
	client *ssh.Client,
	hs tridentutil.HostStatus,
	abActive tridentutil.AbVolumeSelection,
) {
	spec := hs.Spec()

	blkid, err := sysinspect.Blkid(client)
	if err != nil {
		sa.Fail("verity/blkid", err)
		return
	}
	if !blkidHasPath(blkid, "/dev/mapper/root") {
		sa.Failf("verity/mapper", "/dev/mapper/root not present in blkid output")
	}

	// Locate the root verity device from the Host Status.
	rootDeviceID, ok := RootFilesystemDeviceID(spec)
	if !ok {
		sa.Failf("verity/root-mount", "root mount point not found in Host Status spec")
		return
	}
	verity, ok := verityForRoot(spec, rootDeviceID)
	if !ok || verity.HashDeviceID == "" {
		sa.Failf("verity/config", "no verity configuration found for root device %q", rootDeviceID)
		return
	}

	name := verity.Name
	if name == "" {
		name = "root"
	}

	// veritysetup status must show an active, verified, read-only device.
	status, err := sysinspect.VeritySetup(client, name)
	if err != nil {
		sa.Fail("verity/status", err)
		return
	}
	sa.Assert("verity/active", status.Active, "verity device %q is not active/in-use", name)
	assertVerityField(sa, status, "type", "VERITY")
	assertVerityField(sa, status, "status", "verified")
	assertVerityField(sa, status, "mode", "readonly")

	dataDevice, dataOk := status.Get("data device")
	hashDevice, hashOk := status.Get("hash device")
	if !dataOk || !hashOk {
		sa.Failf("verity/devices", "veritysetup status missing data/hash device fields")
		return
	}

	if spec.HasABUpdate() {
		validateVerityAbDevices(sa, client, spec, blkid, verity, dataDevice, hashDevice, abActive)
	} else {
		validateVerityNonAbDevices(sa, client, blkid, verity, dataDevice, hashDevice)
	}
}

// validateVerityAbDevices validates the A/B branch: the veritysetup data/hash
// devices must correspond to the active volume of the data/hash A/B pairs.
func validateVerityAbDevices(
	sa *SoftAsserter,
	client *ssh.Client,
	spec hostconfig.HostConfig,
	blkid map[string]sysinspect.BlkidEntry,
	verity VerityDevice,
	dataDevice, hashDevice string,
	abActive tridentutil.AbVolumeSelection,
) {
	activeDataID, dataFound := ActiveVolumeID(spec, verity.DataDeviceID, abActive)
	activeHashID, hashFound := ActiveVolumeID(spec, verity.HashDeviceID, abActive)
	if !dataFound || !hashFound {
		sa.Failf("verity/ab-pair", "could not resolve active data/hash volume for %s", abActive)
		return
	}

	dataRaid, _, _ := sysinspect.RaidNameForDevice(client, dataDevice)
	hashRaid, _, _ := sysinspect.RaidNameForDevice(client, hashDevice)
	dataIsRaid := dataRaid != ""
	hashIsRaid := hashRaid != ""
	if dataIsRaid != hashIsRaid {
		sa.Failf("verity/ab-raid-parity",
			"data/hash RAID mismatch: data raid=%v hash raid=%v", dataIsRaid, hashIsRaid)
		return
	}

	if dataIsRaid {
		sa.Assert("verity/ab-data-raid", basename(dataRaid) == activeDataID,
			"active data volume %q != raid %q", activeDataID, basename(dataRaid))
		sa.Assert("verity/ab-hash-raid", basename(hashRaid) == activeHashID,
			"active hash volume %q != raid %q", activeHashID, basename(hashRaid))
		return
	}

	// Partition case: PARTLABEL of the block device must equal the active ID.
	assertPartlabelEquals(sa, blkid, dataDevice, activeDataID, "verity/ab-data-partlabel")
	assertPartlabelEquals(sa, blkid, hashDevice, activeHashID, "verity/ab-hash-partlabel")
}

// validateVerityNonAbDevices validates the non-A/B branch: the veritysetup
// data/hash devices must correspond to the configured data/hash device IDs.
func validateVerityNonAbDevices(
	sa *SoftAsserter,
	client *ssh.Client,
	blkid map[string]sysinspect.BlkidEntry,
	verity VerityDevice,
	dataDevice, hashDevice string,
) {
	dataRaid, _, _ := sysinspect.RaidNameForDevice(client, dataDevice)
	hashRaid, _, _ := sysinspect.RaidNameForDevice(client, hashDevice)
	dataIsRaid := dataRaid != ""
	hashIsRaid := hashRaid != ""
	if dataIsRaid != hashIsRaid {
		sa.Failf("verity/raid-parity",
			"data/hash RAID mismatch: data raid=%v hash raid=%v", dataIsRaid, hashIsRaid)
		return
	}

	if dataIsRaid {
		sa.Assert("verity/data-raid", basename(dataRaid) == verity.DataDeviceID,
			"data device id %q != raid %q", verity.DataDeviceID, basename(dataRaid))
		sa.Assert("verity/hash-raid", basename(hashRaid) == verity.HashDeviceID,
			"hash device id %q != raid %q", verity.HashDeviceID, basename(hashRaid))
		return
	}

	// Partition case: base_test's non-A/B branch only asserts presence in blkid.
	if !blkidHasDevice(blkid, basename(dataDevice)) {
		sa.Failf("verity/data-present", "verity data device %q not present in blkid", dataDevice)
	}
	if !blkidHasDevice(blkid, basename(hashDevice)) {
		sa.Failf("verity/hash-present", "verity hash device %q not present in blkid", hashDevice)
	}
}

func assertVerityField(sa *SoftAsserter, status sysinspect.VeritySetupStatus, field, want string) {
	got, ok := status.Get(field)
	sa.Assert("verity/"+field, ok && got == want,
		"veritysetup %s = %q, want %q", field, got, want)
}

func assertPartlabelEquals(sa *SoftAsserter, blkid map[string]sysinspect.BlkidEntry, devicePath, wantLabel, checkName string) {
	entry, ok := blkid[basename(devicePath)]
	if !ok {
		sa.Failf(checkName, "device %q not present in blkid", devicePath)
		return
	}
	label, ok := entry.Get("PARTLABEL")
	sa.Assert(checkName, ok && label == wantLabel,
		"device %q PARTLABEL = %q, want %q", devicePath, label, wantLabel)
}

func blkidHasDevice(blkid map[string]sysinspect.BlkidEntry, deviceBasename string) bool {
	_, ok := blkid[deviceBasename]
	return ok
}

// blkidHasPath reports whether any blkid entry has the given full device path.
func blkidHasPath(blkid map[string]sysinspect.BlkidEntry, path string) bool {
	for _, entry := range blkid {
		if entry.Path == path {
			return true
		}
	}
	return false
}

func basename(path string) string {
	return path[strings.LastIndex(path, "/")+1:]
}
