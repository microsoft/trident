package scenario

import (
	"fmt"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

// validateEncryption validates LUKS2/TPM2 encryption on the remote host.
// Converted from encryption_test.py test_encryption.
//
// For each encryption volume in the host configuration, it validates:
//   - Backing device type (partition or RAID array) and crypto_LUKS type in blkid
//   - cryptsetup luksDump metadata (LUKS2 keyslots, TPM2 tokens, KDF, digest)
//   - cryptsetup status (cipher aes-xts-plain64, keysize 512)
//   - dmsetup info (active state, CRYPT-LUKS2 or CRYPT-PLAIN UUID)
//   - Filesystem mount or swap activation for the decrypted device
//   - A/B volume pair active/inactive state
func (s *TridentE2EScenario) validateEncryption(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	// Get blkid export output for looking up device types and paths
	blkidExportOut, err := sudoCommand(s.sshClient, "blkid --output export")
	if err != nil {
		return fmt.Errorf("failed to run blkid --output export: %w", err)
	}
	blockDevs := ParseBlkidExport(blkidExportOut)

	// After initial install, the active volume is always volume-a.
	abActiveVolume := "volume-a"

	encryptionVolumes := s.originalConfig.S("storage", "encryption", "volumes").Children()
	logrus.Infof("Found %d encryption volumes to validate", len(encryptionVolumes))

	for _, crypt := range encryptionVolumes {
		cryptId, _ := crypt.S("id").Data().(string)
		cryptDevName, _ := crypt.S("deviceName").Data().(string)
		cryptDevId, _ := crypt.S("deviceId").Data().(string)

		logrus.Infof("Validating encryption volume: id=%s, deviceName=%s, deviceId=%s",
			cryptId, cryptDevName, cryptDevId)

		if err := s.checkCryptDevice(tc, abActiveVolume, blockDevs,
			cryptId, cryptDevName, cryptDevId); err != nil {
			return err
		}
	}

	logrus.Info("Encryption validation passed")
	return nil
}

// checkCryptDevice validates a single encryption volume. It checks the backing
// device, LUKS metadata, cryptsetup status, dmsetup info, and filesystem mount
// or swap activation.
func (s *TridentE2EScenario) checkCryptDevice(
	tc storm.TestCase,
	abActiveVolume string,
	blockDevs map[string]map[string]string,
	cryptId, cryptDevName, cryptDevId string,
) error {
	cryptDevicePath := fmt.Sprintf("/dev/mapper/%s", cryptDevName)

	// Validate backing device and LUKS dump
	if err := s.checkParentDevices(tc, blockDevs, cryptDevId); err != nil {
		return err
	}

	isSwap := false
	isInUse := true

	// Check if this crypt volume is part of an A/B update volume pair
	volumePairId, isVolumeA, hasABPair := s.getChildABUpdateVolumePair(cryptId)

	if hasABPair {
		if abActiveVolume != "volume-a" && abActiveVolume != "volume-b" {
			tc.Fail(fmt.Sprintf("expected active volume to be 'volume-a' or 'volume-b', got %q",
				abActiveVolume))
			return nil
		}

		isInUse = (abActiveVolume == "volume-a" && isVolumeA) ||
			(abActiveVolume == "volume-b" && !isVolumeA)

		mpPath := s.getFilesystemMountPath(volumePairId)
		if mpPath == "" {
			tc.Fail(fmt.Sprintf("no filesystem/mount point found for A/B volume pair %q",
				volumePairId))
			return nil
		}

		if err := CheckPathExists(s.sshClient, mpPath); err != nil {
			tc.Fail(fmt.Sprintf("mount path %q does not exist: %v", mpPath, err))
			return nil
		}

		if err := s.checkFindmnt(tc, mpPath, cryptDevicePath, isInUse); err != nil {
			return err
		}
	} else if s.isSwapDevice(cryptId) {
		isSwap = true

		activeSwaps, err := s.getActiveSwaps()
		if err != nil {
			return fmt.Errorf("failed to get active swaps: %w", err)
		}

		realPath, err := sudoCommand(s.sshClient,
			fmt.Sprintf("readlink -f %s", cryptDevicePath))
		if err != nil {
			return fmt.Errorf("failed to resolve %s: %w", cryptDevicePath, err)
		}

		if !activeSwaps[realPath] {
			tc.Fail(fmt.Sprintf("expected %q to be in active swaps: %v",
				realPath, activeSwaps))
			return nil
		}
	} else {
		// Regular filesystem (not A/B, not swap)
		mpPath := s.getFilesystemMountPath(cryptId)
		if mpPath == "" {
			tc.Fail(fmt.Sprintf("no filesystem/mount point found for encryption volume %q",
				cryptId))
			return nil
		}

		if err := CheckPathExists(s.sshClient, mpPath); err != nil {
			tc.Fail(fmt.Sprintf("mount path %q does not exist: %v", mpPath, err))
			return nil
		}

		if err := s.checkFindmnt(tc, mpPath, cryptDevicePath, isInUse); err != nil {
			return err
		}
	}

	// Verify the device mapper path exists
	if err := CheckPathExists(s.sshClient, cryptDevicePath); err != nil {
		tc.Fail(fmt.Sprintf("crypt device path %q does not exist: %v",
			cryptDevicePath, err))
		return nil
	}

	// Validate cryptsetup status
	if err := s.checkCryptsetupStatus(tc, cryptDevName, isInUse); err != nil {
		return err
	}

	// Validate dmsetup info
	return s.checkDmsetupInfo(tc, cryptDevName, isSwap)
}

// checkParentDevices validates the backing device for an encryption volume.
// The backing device can be either a disk partition or a RAID array. It also
// validates the device type is crypto_LUKS and checks the LUKS dump metadata.
func (s *TridentE2EScenario) checkParentDevices(
	tc storm.TestCase,
	blockDevs map[string]map[string]string,
	cryptDevId string,
) error {
	var cryptDevPath string

	if s.isDiskPartition(cryptDevId) {
		cryptDevPath = getBlockDevPathByPartlabel(blockDevs, cryptDevId)
		if cryptDevPath == "" {
			tc.Fail(fmt.Sprintf("expected device with PARTLABEL %q in blkid export output",
				cryptDevId))
			return nil
		}
	} else {
		raidName := s.getRaidSoftwareArrayName(cryptDevId)
		if raidName == "" {
			tc.Fail(fmt.Sprintf("expected %q to be a disk partition or RAID array",
				cryptDevId))
			return nil
		}

		resolvedPath, err := sudoCommand(s.sshClient,
			fmt.Sprintf("readlink -f /dev/md/%s", raidName))
		if err != nil {
			return fmt.Errorf("failed to resolve RAID device path for %q: %w",
				raidName, err)
		}
		cryptDevPath = resolvedPath
	}

	// Validate that the device type is crypto_LUKS
	devProps, exists := blockDevs[cryptDevPath]
	if !exists {
		tc.Fail(fmt.Sprintf("device %q not found in blkid export output", cryptDevPath))
		return nil
	}

	if actualType := devProps["TYPE"]; actualType != "crypto_LUKS" {
		tc.Fail(fmt.Sprintf("expected TYPE 'crypto_LUKS' for %q, got %q",
			cryptDevPath, actualType))
		return nil
	}

	return s.checkCryptsetupLuksDump(tc, cryptDevPath)
}

// checkCryptsetupLuksDump validates LUKS2 metadata from cryptsetup luksDump.
// It verifies keyslots, tokens, KDF, digests, and UKI vs non-UKI PCR policies.
func (s *TridentE2EScenario) checkCryptsetupLuksDump(
	tc storm.TestCase,
	cryptDevPath string,
) error {
	// SELinux workaround: luksDump needs additional lvm_t permissions that
	// are a test-infra quirk, not part of the Trident SELinux policy.
	enforcing, err := sudoCommand(s.sshClient, "getenforce")
	if err != nil {
		return fmt.Errorf("failed to check SELinux status: %w", err)
	}
	isEnforcing := strings.TrimSpace(enforcing) == "Enforcing"

	if isEnforcing {
		if _, err := sudoCommand(s.sshClient, "setenforce 0"); err != nil {
			return fmt.Errorf("failed to set SELinux to permissive: %w", err)
		}
	}

	luksDumpOut, err := sudoCommand(s.sshClient,
		fmt.Sprintf("cryptsetup luksDump --dump-json-metadata %s", cryptDevPath))

	if isEnforcing {
		if _, restoreErr := sudoCommand(s.sshClient, "setenforce 1"); restoreErr != nil {
			logrus.WithError(restoreErr).Warn("Failed to restore SELinux to enforcing")
		}
	}

	if err != nil {
		return fmt.Errorf("failed to run cryptsetup luksDump on %s: %w",
			cryptDevPath, err)
	}

	dump, err := ParseLuksDump(luksDumpOut)
	if err != nil {
		return fmt.Errorf("failed to parse luksDump output for %s: %w",
			cryptDevPath, err)
	}

	// --- Validate digests ---
	digest0, ok := dump.Digests["0"]
	if !ok {
		tc.Fail(fmt.Sprintf("expected digest 0 in luksDump for %s", cryptDevPath))
		return nil
	}
	if digest0.Type != "pbkdf2" {
		tc.Fail(fmt.Sprintf("expected digest type 'pbkdf2', got %q", digest0.Type))
		return nil
	}
	if digest0.Hash != "sha512" {
		tc.Fail(fmt.Sprintf("expected digest hash 'sha512', got %q", digest0.Hash))
		return nil
	}

	// --- Validate tokens ---
	token0, ok := dump.Tokens["0"]
	if !ok {
		tc.Fail(fmt.Sprintf("expected token 0 in luksDump for %s", cryptDevPath))
		return nil
	}
	if len(dump.Tokens) != 1 {
		tc.Fail(fmt.Sprintf("expected 1 token, got %d", len(dump.Tokens)))
		return nil
	}
	if len(token0.Keyslots) != 1 || token0.Keyslots[0] != "1" {
		tc.Fail(fmt.Sprintf("expected token 0 keyslots [\"1\"], got %v",
			token0.Keyslots))
		return nil
	}
	if token0.Type != "systemd-tpm2" {
		tc.Fail(fmt.Sprintf("expected token type 'systemd-tpm2', got %q", token0.Type))
		return nil
	}

	// UKI vs non-UKI PCR policy validation
	if s.configParams.IsUki {
		if token0.TPM2PCRLock == nil || !*token0.TPM2PCRLock {
			tc.Fail("expected tpm2_pcrlock to be true for UKI image")
			return nil
		}
		if len(token0.TPM2PCRs) != 0 {
			tc.Fail(fmt.Sprintf("expected empty tpm2-pcrs for UKI image, got %v",
				token0.TPM2PCRs))
			return nil
		}
	} else {
		if token0.TPM2PCRLock != nil && *token0.TPM2PCRLock {
			tc.Fail("expected tpm2_pcrlock to be false for non-UKI image")
			return nil
		}
		if len(token0.TPM2PCRs) != 1 || token0.TPM2PCRs[0] != 7 {
			tc.Fail(fmt.Sprintf("expected tpm2-pcrs [7] for non-UKI image, got %v",
				token0.TPM2PCRs))
			return nil
		}
	}

	// --- Validate keyslots ---
	if len(dump.Keyslots) != 1 {
		tc.Fail(fmt.Sprintf("expected 1 keyslot, got %d", len(dump.Keyslots)))
		return nil
	}
	keyslot1, ok := dump.Keyslots["1"]
	if !ok {
		tc.Fail("expected keyslot 1 in luksDump")
		return nil
	}
	if keyslot1.Type != "luks2" {
		tc.Fail(fmt.Sprintf("expected keyslot type 'luks2', got %q", keyslot1.Type))
		return nil
	}
	if keyslot1.KDF.Type != "pbkdf2" {
		tc.Fail(fmt.Sprintf("expected keyslot KDF type 'pbkdf2', got %q",
			keyslot1.KDF.Type))
		return nil
	}
	if keyslot1.KDF.Hash != "sha512" {
		tc.Fail(fmt.Sprintf("expected keyslot KDF hash 'sha512', got %q",
			keyslot1.KDF.Hash))
		return nil
	}
	if keyslot1.Area.Encryption != "aes-xts-plain64" {
		tc.Fail(fmt.Sprintf("expected keyslot area encryption 'aes-xts-plain64', got %q",
			keyslot1.Area.Encryption))
		return nil
	}

	logrus.Infof("LUKS dump validation passed for %s", cryptDevPath)
	return nil
}

// checkCryptsetupStatus validates the output of `cryptsetup status` for an
// encrypted device. Checks the active/in-use status line, cipher, and keysize.
func (s *TridentE2EScenario) checkCryptsetupStatus(
	tc storm.TestCase,
	name string,
	isInUse bool,
) error {
	stdout, err := sudoCommand(s.sshClient,
		fmt.Sprintf("cryptsetup status %s", name))
	if err != nil {
		return fmt.Errorf("failed to run cryptsetup status %s: %w", name, err)
	}

	lines := strings.SplitN(stdout, "\n", 2)
	if len(lines) == 0 {
		tc.Fail(fmt.Sprintf("empty output from cryptsetup status %s", name))
		return nil
	}

	firstLine := strings.TrimSpace(lines[0])
	if isInUse {
		expected := fmt.Sprintf("/dev/mapper/%s is active and is in use.", name)
		if firstLine != expected {
			tc.Fail(fmt.Sprintf("expected %q, got %q", expected, firstLine))
			return nil
		}
	} else {
		expected := fmt.Sprintf("/dev/mapper/%s is active.", name)
		if firstLine != expected {
			tc.Fail(fmt.Sprintf("expected %q, got %q", expected, firstLine))
			return nil
		}
	}

	status := ParseCryptsetupStatus(stdout)

	if status.Cipher != "aes-xts-plain64" {
		tc.Fail(fmt.Sprintf("expected cipher 'aes-xts-plain64', got %q", status.Cipher))
		return nil
	}
	if status.Keysize != "512 bits" {
		tc.Fail(fmt.Sprintf("expected keysize '512 bits', got %q", status.Keysize))
		return nil
	}

	logrus.Infof("Cryptsetup status validation passed for %s", name)
	return nil
}

// checkDmsetupInfo validates the output of `dmsetup info` for an encrypted
// device. Checks the name, state, tables, and UUID format.
func (s *TridentE2EScenario) checkDmsetupInfo(
	tc storm.TestCase,
	name string,
	isSwap bool,
) error {
	stdout, err := sudoCommand(s.sshClient,
		fmt.Sprintf("dmsetup info %s", name))
	if err != nil {
		return fmt.Errorf("failed to run dmsetup info %s: %w", name, err)
	}

	info := ParseDmsetupInfo(stdout)

	if info.Name != name {
		tc.Fail(fmt.Sprintf("expected Name %q, got %q", name, info.Name))
		return nil
	}
	if info.State != "ACTIVE" {
		tc.Fail(fmt.Sprintf("expected State 'ACTIVE', got %q", info.State))
		return nil
	}
	if info.TablesPresent != "LIVE" {
		tc.Fail(fmt.Sprintf("expected Tables present 'LIVE', got %q", info.TablesPresent))
		return nil
	}

	cryptKind := "LUKS2"
	if isSwap {
		cryptKind = "PLAIN"
	}

	expectedPrefix := fmt.Sprintf("CRYPT-%s-", cryptKind)
	if !strings.HasPrefix(info.UUID, expectedPrefix) {
		tc.Fail(fmt.Sprintf("expected UUID prefix %q, got %q", expectedPrefix, info.UUID))
		return nil
	}

	expectedSuffix := fmt.Sprintf("-%s", name)
	if !strings.HasSuffix(info.UUID, expectedSuffix) {
		tc.Fail(fmt.Sprintf("expected UUID suffix %q, got %q", expectedSuffix, info.UUID))
		return nil
	}

	logrus.Infof("Dmsetup info validation passed for %s", name)
	return nil
}

// checkFindmnt validates the mount point for an encrypted device using findmnt.
func (s *TridentE2EScenario) checkFindmnt(
	tc storm.TestCase,
	target, source string,
	isActive bool,
) error {
	stdout, err := sudoCommand(s.sshClient, fmt.Sprintf("findmnt %s", target))
	if err != nil {
		return fmt.Errorf("failed to run findmnt %s: %w", target, err)
	}

	table := ParseTable(stdout)
	if len(table) != 1 {
		tc.Fail(fmt.Sprintf("expected 1 findmnt row for %s, got %d", target, len(table)))
		return nil
	}

	row := table[0]
	if row["TARGET"] != target {
		tc.Fail(fmt.Sprintf("expected TARGET %q, got %q", target, row["TARGET"]))
		return nil
	}
	if row["FSTYPE"] != "ext4" {
		tc.Fail(fmt.Sprintf("expected FSTYPE 'ext4' for %s, got %q", target, row["FSTYPE"]))
		return nil
	}

	if isActive {
		if row["SOURCE"] != source {
			tc.Fail(fmt.Sprintf("expected SOURCE %q when active, got %q", source, row["SOURCE"]))
			return nil
		}
	} else {
		if row["SOURCE"] == source {
			tc.Fail(fmt.Sprintf("expected SOURCE different from %q when inactive", source))
			return nil
		}
	}

	return nil
}

// --- Host config accessor helpers ---

// getChildABUpdateVolumePair checks if the given crypt ID is a member of an
// A/B update volume pair. Returns the volume pair ID, whether the crypt ID
// is volume A, and whether a pair was found.
func (s *TridentE2EScenario) getChildABUpdateVolumePair(cryptId string) (string, bool, bool) {
	for _, vp := range s.originalConfig.S("storage", "abUpdate", "volumePairs").Children() {
		volumeAId, _ := vp.S("volumeAId").Data().(string)
		volumeBId, _ := vp.S("volumeBId").Data().(string)

		if volumeAId == cryptId {
			vpId, _ := vp.S("id").Data().(string)
			return vpId, true, true
		}
		if volumeBId == cryptId {
			vpId, _ := vp.S("id").Data().(string)
			return vpId, false, true
		}
	}
	return "", false, false
}

// getFilesystemMountPath finds the mount point path for a filesystem by its
// deviceId. The mountPoint can be either a string or an object with a "path" key.
func (s *TridentE2EScenario) getFilesystemMountPath(fsId string) string {
	for _, fs := range s.originalConfig.S("storage", "filesystems").Children() {
		deviceId, _ := fs.S("deviceId").Data().(string)
		if deviceId != fsId {
			continue
		}

		mpData := fs.S("mountPoint").Data()
		if mpPath, ok := mpData.(string); ok {
			return mpPath
		}
		if mpPath, ok := fs.S("mountPoint", "path").Data().(string); ok {
			return mpPath
		}
	}
	return ""
}

// isSwapDevice checks if the given device ID is configured as a swap device.
// Swap entries can be either a bare string (deviceId) or an object with a
// "deviceId" key.
func (s *TridentE2EScenario) isSwapDevice(devId string) bool {
	for _, swapItem := range s.originalConfig.S("storage", "swap").Children() {
		data := swapItem.Data()
		if str, ok := data.(string); ok && str == devId {
			return true
		}
		if did, ok := swapItem.S("deviceId").Data().(string); ok && did == devId {
			return true
		}
	}
	return false
}

// getActiveSwaps returns a set of resolved paths for active swap devices.
func (s *TridentE2EScenario) getActiveSwaps() (map[string]bool, error) {
	stdout, err := sudoCommand(s.sshClient,
		"swapon --show=NAME --raw --bytes --noheadings | xargs -I @ readlink -f @")
	if err != nil {
		return nil, fmt.Errorf("failed to get active swaps: %w", err)
	}

	swaps := make(map[string]bool)
	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line != "" {
			swaps[line] = true
		}
	}
	return swaps, nil
}

// isDiskPartition checks if a device ID refers to a disk partition in the host
// configuration.
func (s *TridentE2EScenario) isDiskPartition(pId string) bool {
	for _, disk := range s.originalConfig.S("storage", "disks").Children() {
		for _, part := range disk.S("partitions").Children() {
			id, _ := part.S("id").Data().(string)
			if id == pId {
				return true
			}
		}
	}
	return false
}

// getRaidSoftwareArrayName returns the RAID array name for the given array ID,
// or empty string if not found.
func (s *TridentE2EScenario) getRaidSoftwareArrayName(aId string) string {
	for _, arr := range s.originalConfig.S("storage", "raid", "software").Children() {
		id, _ := arr.S("id").Data().(string)
		if id == aId {
			name, _ := arr.S("name").Data().(string)
			return name
		}
	}
	return ""
}

// getBlockDevPathByPartlabel finds a device path by PARTLABEL from blkid
// export output.
func getBlockDevPathByPartlabel(blockDevs map[string]map[string]string, label string) string {
	for devPath, props := range blockDevs {
		if props["PARTLABEL"] == label {
			return devPath
		}
	}
	return ""
}
