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

const (
	expectedCipher     = "aes-xts-plain64"
	expectedKeysize    = "512 bits"
	expectedFsType     = "ext4"
	expectedLuksType   = "crypto_LUKS"
	expectedDigestType = "pbkdf2"
	expectedDigestHash = "sha512"
)

// HasEncryption reports whether the spec declares encryption volumes, used to
// self-select the encryption validation.
func HasEncryption(hs tridentutil.HostStatus) bool {
	return hs.Spec().Exists("storage", "encryption")
}

// ValidateEncryption ports encryption_test.py::test_encryption. For each
// configured encryption volume it validates the backing device is LUKS, the
// LUKS metadata, the device-mapper state, and the mount/swap/active status
// (accounting for A/B update pairs). isUki selects the expected TPM2 policy.
func ValidateEncryption(
	sa *SoftAsserter,
	client *ssh.Client,
	hs tridentutil.HostStatus,
	isUki bool,
	abActive tridentutil.AbVolumeSelection,
) {
	spec := hs.Spec()

	blockDevs, err := sysinspect.BlkidExport(client)
	if err != nil {
		sa.Fail("encryption/blkid-export", err)
		return
	}

	for _, crypt := range spec.S("storage", "encryption", "volumes").Children() {
		cryptID, _ := crypt.S("id").Data().(string)
		cryptDevName, _ := crypt.S("deviceName").Data().(string)
		cryptDevID, _ := crypt.S("deviceId").Data().(string)
		checkCryptDevice(sa, client, spec, isUki, abActive, blockDevs, cryptID, cryptDevName, cryptDevID)
	}
}

func checkCryptDevice(
	sa *SoftAsserter,
	client *ssh.Client,
	spec hostconfig.HostConfig,
	isUki bool,
	abActive tridentutil.AbVolumeSelection,
	blockDevs map[string]map[string]string,
	cryptID, cryptDevName, cryptDevID string,
) {
	cryptDevicePath := "/dev/mapper/" + cryptDevName

	checkParentDevices(sa, client, spec, isUki, blockDevs, cryptDevID)

	swap := false
	isInUse := true

	if pair, isVolumeA, ok := childAbUpdateVolumePair(spec, cryptID); ok {
		// Encryption volume is an A/B pair member: it is in use only when its
		// side matches the active volume.
		isInUse = (abActive == tridentutil.AbVolumeA && isVolumeA) ||
			(abActive == tridentutil.AbVolumeB && !isVolumeA)

		pairID, _ := pair.S("id").Data().(string)
		fs, ok := filesystemByDeviceID(spec, pairID)
		if !ok {
			sa.Failf("encryption/ab-fs", "no filesystem for A/B volume pair %q", pairID)
		} else if mp, ok := mountPointPath(&fs); ok {
			checkExists(sa, client, mp)
			checkFindmnt(sa, client, mp, cryptDevicePath, isInUse)
		} else {
			sa.Failf("encryption/ab-mount", "no mount point for A/B volume pair %q", pairID)
		}
	} else if isSwapDevice(spec, cryptID) {
		swap = true
		swaps, err := sysinspect.ActiveSwaps(client)
		if err != nil {
			sa.Fail("encryption/swaps", err)
		} else {
			realPath, err := sysinspect.ReadlinkF(client, cryptDevicePath)
			if err != nil {
				sa.Fail("encryption/swap-readlink", err)
			} else if _, active := swaps[realPath]; !active {
				sa.Failf("encryption/swap-active", "swap %q not in active swaps %v", realPath, swaps)
			}
		}
	} else {
		fs, ok := filesystemByDeviceID(spec, cryptID)
		if !ok {
			sa.Failf("encryption/fs", "no filesystem for encryption volume %q", cryptID)
		} else if mp, ok := mountPointPath(&fs); ok {
			checkExists(sa, client, mp)
			checkFindmnt(sa, client, mp, cryptDevicePath, isInUse)
		} else {
			sa.Failf("encryption/mount", "encryption volume %q filesystem is not mounted", cryptID)
		}
	}

	checkExists(sa, client, cryptDevicePath)
	checkCryptsetupStatus(sa, client, cryptDevName, isInUse)
	checkDmsetupInfo(sa, client, cryptDevName, swap)
}

// checkParentDevices confirms the crypt device's backing block device is LUKS
// and validates its LUKS metadata.
func checkParentDevices(
	sa *SoftAsserter,
	client *ssh.Client,
	spec hostconfig.HostConfig,
	isUki bool,
	blockDevs map[string]map[string]string,
	cryptDevID string,
) {
	var cryptDevPath string
	if IsPartition(spec, cryptDevID) {
		path, ok := blockDevPathByPartlabel(blockDevs, cryptDevID)
		if !ok {
			sa.Failf("encryption/parent-partition", "no device with PARTLABEL %q", cryptDevID)
			return
		}
		cryptDevPath = path
	} else {
		raidName, ok := raidSoftwareArrayName(spec, cryptDevID)
		if !ok {
			sa.Failf("encryption/parent-kind", "%q is neither a partition nor a RAID array", cryptDevID)
			return
		}
		path, err := sysinspect.ReadlinkF(client, "/dev/md/"+raidName)
		if err != nil {
			sa.Fail("encryption/parent-raid", err)
			return
		}
		cryptDevPath = path
	}

	dev, ok := blockDevs[cryptDevPath]
	if !ok {
		sa.Failf("encryption/parent-blkid", "no blkid entry for %q", cryptDevPath)
		return
	}
	sa.Assert("encryption/parent-type", dev["TYPE"] == expectedLuksType,
		"device %q TYPE = %q, want %q", cryptDevPath, dev["TYPE"], expectedLuksType)

	checkLuksDump(sa, client, cryptDevPath, isUki)
}

// checkLuksDump validates the LUKS2 metadata from cryptsetup luksDump.
func checkLuksDump(sa *SoftAsserter, client *ssh.Client, cryptDevPath string, isUki bool) {
	// luksDump needs an SELinux permission the Trident policy intentionally
	// omits; temporarily drop to permissive (a testing-infra quirk), matching
	// encryption_test.py.
	enforcing := false
	if mode, err := sysinspect.Getenforce(client); err == nil && mode == "Enforcing" {
		enforcing = true
		if err := sysinspect.Setenforce(client, false); err != nil {
			sa.Fail("encryption/selinux", err)
		}
	}

	dump, err := sysinspect.CryptsetupLuksDump(client, cryptDevPath)

	if enforcing {
		// Best-effort restore of enforcing mode.
		if restoreErr := sysinspect.Setenforce(client, true); restoreErr != nil {
			sa.Fail("encryption/selinux-restore", restoreErr)
		}
	}

	if err != nil {
		sa.Fail("encryption/luks-dump", err)
		return
	}

	digest, ok := dump.Digests["0"]
	if !ok {
		sa.Failf("encryption/luks-digest", "luksDump missing digest 0")
	} else {
		sa.Assert("encryption/luks-digest-type", digest.Type == expectedDigestType,
			"digest type = %q, want %q", digest.Type, expectedDigestType)
		sa.Assert("encryption/luks-digest-hash", digest.Hash == expectedDigestHash,
			"digest hash = %q, want %q", digest.Hash, expectedDigestHash)
	}

	token, ok := dump.Tokens["0"]
	if !ok {
		sa.Failf("encryption/luks-token", "luksDump missing token 0")
	} else {
		sa.Assert("encryption/luks-token-count", len(dump.Tokens) == 1,
			"expected 1 token, got %d", len(dump.Tokens))
		sa.Assert("encryption/luks-token-keyslots", len(token.Keyslots) == 1 && contains(token.Keyslots, "1"),
			"expected token keyslot [1], got %v", token.Keyslots)
		sa.Assert("encryption/luks-token-type", token.Type == "systemd-tpm2",
			"token type = %q, want systemd-tpm2", token.Type)
		if isUki {
			sa.Assert("encryption/luks-pcrlock", token.Tpm2Pcrlock,
				"expected tpm2_pcrlock=true for UKI image")
			sa.Assert("encryption/luks-pcrs", len(token.Tpm2Pcrs) == 0,
				"expected empty tpm2-pcrs for UKI image, got %v", token.Tpm2Pcrs)
		} else {
			sa.Assert("encryption/luks-pcrlock", !token.Tpm2Pcrlock,
				"expected tpm2_pcrlock=false for non-UKI image")
			sa.Assert("encryption/luks-pcrs", len(token.Tpm2Pcrs) == 1 && token.Tpm2Pcrs[0] == 7,
				"expected tpm2-pcrs=[7] for non-UKI image, got %v", token.Tpm2Pcrs)
		}
	}

	keyslot, ok := dump.Keyslots["1"]
	if !ok {
		sa.Failf("encryption/luks-keyslot", "luksDump missing keyslot 1")
	} else {
		sa.Assert("encryption/luks-keyslot-count", len(dump.Keyslots) == 1,
			"expected 1 keyslot, got %d", len(dump.Keyslots))
		sa.Assert("encryption/luks-keyslot-type", keyslot.Type == "luks2",
			"keyslot type = %q, want luks2", keyslot.Type)
		sa.Assert("encryption/luks-kdf-type", keyslot.Kdf.Type == "pbkdf2",
			"keyslot kdf type = %q, want pbkdf2", keyslot.Kdf.Type)
		sa.Assert("encryption/luks-kdf-hash", keyslot.Kdf.Hash == "sha512",
			"keyslot kdf hash = %q, want sha512", keyslot.Kdf.Hash)
		sa.Assert("encryption/luks-area-enc", keyslot.Area.Encryption == expectedCipher,
			"keyslot area encryption = %q, want %q", keyslot.Area.Encryption, expectedCipher)
	}
}

func checkCryptsetupStatus(sa *SoftAsserter, client *ssh.Client, name string, isInUse bool) {
	status, err := sysinspect.Cryptsetup(client, name)
	if err != nil {
		sa.Fail("encryption/cryptsetup-status", err)
		return
	}
	if isInUse {
		sa.Assert("encryption/cryptsetup-inuse", status.InUse,
			"expected %q to be active and in use", name)
	} else {
		sa.Assert("encryption/cryptsetup-active", status.Active && !status.InUse,
			"expected %q to be active but not in use", name)
	}
	if cipher, _ := status.Get("cipher"); cipher != expectedCipher {
		sa.Failf("encryption/cryptsetup-cipher", "cipher = %q, want %q", cipher, expectedCipher)
	}
	if keysize, _ := status.Get("keysize"); keysize != expectedKeysize {
		sa.Failf("encryption/cryptsetup-keysize", "keysize = %q, want %q", keysize, expectedKeysize)
	}
}

func checkDmsetupInfo(sa *SoftAsserter, client *ssh.Client, name string, swap bool) {
	info, err := sysinspect.DmsetupInfo(client, name)
	if err != nil {
		sa.Fail("encryption/dmsetup", err)
		return
	}
	sa.Assert("encryption/dmsetup-name", info["Name"] == name,
		"dmsetup Name = %q, want %q", info["Name"], name)
	sa.Assert("encryption/dmsetup-state", info["State"] == "ACTIVE",
		"dmsetup State = %q, want ACTIVE", info["State"])
	sa.Assert("encryption/dmsetup-tables", info["Tables present"] == "LIVE",
		"dmsetup Tables present = %q, want LIVE", info["Tables present"])

	cryptKind := "LUKS2"
	if swap {
		cryptKind = "PLAIN"
	}
	prefix := fmt.Sprintf("CRYPT-%s-", cryptKind)
	suffix := "-" + name
	uuid := info["UUID"]
	sa.Assert("encryption/dmsetup-uuid-prefix", strings.HasPrefix(uuid, prefix),
		"dmsetup UUID %q does not start with %q", uuid, prefix)
	sa.Assert("encryption/dmsetup-uuid-suffix", strings.HasSuffix(uuid, suffix),
		"dmsetup UUID %q does not end with %q", uuid, suffix)
}

// checkExists runs `sudo ls <path>` and records a failure if it does not exist.
func checkExists(sa *SoftAsserter, client *ssh.Client, path string) {
	out, err := sshutils.RunCommand(client, fmt.Sprintf("sudo ls %s", path))
	if err != nil {
		sa.Fail("encryption/exists", err)
		return
	}
	sa.Assert("encryption/exists", out.Status == 0, "path does not exist: %s", path)
}

// checkFindmnt validates the findmnt row for target: when active, SOURCE must be
// the crypt device; when inactive, SOURCE must differ. FSTYPE is always ext4.
func checkFindmnt(sa *SoftAsserter, client *ssh.Client, target, source string, isActive bool) {
	rows, err := sysinspect.Findmnt(client, target)
	if err != nil {
		sa.Fail("encryption/findmnt", err)
		return
	}
	if len(rows) != 1 {
		sa.Failf("encryption/findmnt-rows", "expected 1 findmnt row for %q, got %d", target, len(rows))
		return
	}
	row := rows[0]
	sa.Assert("encryption/findmnt-target", row.Target == target,
		"findmnt TARGET = %q, want %q", row.Target, target)
	sa.Assert("encryption/findmnt-fstype", row.FsType == expectedFsType,
		"findmnt FSTYPE = %q, want %q", row.FsType, expectedFsType)
	if isActive {
		sa.Assert("encryption/findmnt-source", row.Source == source,
			"findmnt SOURCE = %q, want %q (active)", row.Source, source)
	} else {
		sa.Assert("encryption/findmnt-source", row.Source != source,
			"findmnt SOURCE = %q, expected different from %q (inactive)", row.Source, source)
	}
}

// --- Host Configuration helpers (operate on the Host Status spec) ---

// childAbUpdateVolumePair returns the A/B volume pair that has cryptID as one of
// its volumes, whether cryptID is the A side, and whether such a pair exists.
func childAbUpdateVolumePair(spec hostconfig.HostConfig, cryptID string) (*hostconfig.HostConfig, bool, bool) {
	for _, pair := range spec.S("storage", "abUpdate", "volumePairs").Children() {
		if a, _ := pair.S("volumeAId").Data().(string); a == cryptID {
			hc := hostconfig.NewHostConfigFromContainer(pair)
			return &hc, true, true
		}
		if b, _ := pair.S("volumeBId").Data().(string); b == cryptID {
			hc := hostconfig.NewHostConfigFromContainer(pair)
			return &hc, false, true
		}
	}
	return nil, false, false
}

// filesystemByDeviceID returns the filesystem with the given deviceId.
func filesystemByDeviceID(spec hostconfig.HostConfig, deviceID string) (hostconfig.HostConfig, bool) {
	for _, fs := range spec.S("storage", "filesystems").Children() {
		if id, _ := fs.S("deviceId").Data().(string); id == deviceID {
			return hostconfig.NewHostConfigFromContainer(fs), true
		}
	}
	return hostconfig.HostConfig{}, false
}

// isSwapDevice reports whether devID is configured as swap. storage.swap entries
// may be plain device-id strings or objects with a deviceId field.
func isSwapDevice(spec hostconfig.HostConfig, devID string) bool {
	for _, swap := range spec.S("storage", "swap").Children() {
		if s, ok := swap.Data().(string); ok && s == devID {
			return true
		}
		if id, ok := swap.S("deviceId").Data().(string); ok && id == devID {
			return true
		}
	}
	return false
}

// raidSoftwareArrayName returns the name of the software RAID array with the
// given id.
func raidSoftwareArrayName(spec hostconfig.HostConfig, id string) (string, bool) {
	for _, raid := range spec.S("storage", "raid", "software").Children() {
		if rid, _ := raid.S("id").Data().(string); rid == id {
			if name, ok := raid.S("name").Data().(string); ok {
				return name, true
			}
		}
	}
	return "", false
}

// blockDevPathByPartlabel returns the device path whose PARTLABEL matches label.
func blockDevPathByPartlabel(blockDevs map[string]map[string]string, label string) (string, bool) {
	for path, dev := range blockDevs {
		if dev["PARTLABEL"] == label {
			return path, true
		}
	}
	return "", false
}

// contains reports whether s contains v.
func contains(s []string, v string) bool {
	for _, x := range s {
		if x == v {
			return true
		}
	}
	return false
}
