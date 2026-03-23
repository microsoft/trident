package scenario

import (
	"fmt"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/utils/trident"
)

// validateVerity validates DM-Verity root filesystem configuration on the
// remote host. Converted from verity_test.py test_verity_root.
//
// It validates:
//   - /dev/mapper/root exists in blkid output
//   - veritysetup status reports type VERITY, status verified, mode readonly
//   - Data and hash device mapping matches host status verity configuration
//   - A/B active volume matches expected block devices (partition or RAID)
func (s *TridentE2EScenario) validateVerity(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	// --- 1. Run blkid and verify /dev/mapper/root exists ---
	blkidOut, err := sudoCommand(s.sshClient, "blkid")
	if err != nil {
		return fmt.Errorf("failed to run blkid: %w", err)
	}

	blkidEntries := ParseBlkid(blkidOut)

	if _, ok := blkidEntries["root"]; !ok {
		tc.Fail("/dev/mapper/root not found in blkid output")
		return nil
	}

	// Build a map from short device name to PARTLABEL for later device matching
	partitionsByLabel := make(map[string]BlkidEntry)
	for name, entry := range blkidEntries {
		partitionsByLabel[name] = entry
	}

	// --- 2. Run veritysetup status and validate key properties ---
	verityStatusOut, err := sudoCommand(s.sshClient, "veritysetup status root")
	if err != nil {
		return fmt.Errorf("failed to run veritysetup status root: %w", err)
	}

	verityStatus := ParseVeritySetupStatus(verityStatusOut)

	if !verityStatus.IsActive || !verityStatus.IsInUse {
		tc.Fail(fmt.Sprintf("expected /dev/mapper/root to be active and in use, got: %q",
			verityStatus.StatusLine))
		return nil
	}

	if verityStatus.Properties["type"] != "VERITY" {
		tc.Fail(fmt.Sprintf("expected veritysetup type 'VERITY', got %q",
			verityStatus.Properties["type"]))
		return nil
	}

	if verityStatus.Properties["status"] != "verified" {
		tc.Fail(fmt.Sprintf("expected veritysetup status 'verified', got %q",
			verityStatus.Properties["status"]))
		return nil
	}

	if verityStatus.Properties["mode"] != "readonly" {
		tc.Fail(fmt.Sprintf("expected veritysetup mode 'readonly', got %q",
			verityStatus.Properties["mode"]))
		return nil
	}

	logrus.Info("Verity status validation passed (type=VERITY, status=verified, mode=readonly)")

	// --- 3. Get host status via trident get ---
	tridentOut, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "get")
	if err != nil {
		return fmt.Errorf("failed to run trident get: %w", err)
	}

	if tridentOut.Status != 0 {
		return fmt.Errorf("trident get failed with status %d: %s",
			tridentOut.Status, tridentOut.Stderr)
	}

	hostStatus, err := ParseTridentGetOutput(tridentOut.Stdout)
	if err != nil {
		return fmt.Errorf("failed to parse trident get output: %w", err)
	}

	// --- 4. Find root mount filesystem and its verity device ---
	spec, _ := hostStatus["spec"].(map[interface{}]interface{})
	if spec == nil {
		tc.Fail("no spec found in host status")
		return nil
	}

	storage, _ := spec["storage"].(map[interface{}]interface{})
	if storage == nil {
		tc.Fail("no storage found in host status spec")
		return nil
	}

	// Find the filesystem with mountPoint path "/"
	var rootMountID string
	filesystems, _ := storage["filesystems"].([]interface{})
	for _, fsRaw := range filesystems {
		fs, _ := fsRaw.(map[interface{}]interface{})
		if fs == nil {
			continue
		}
		mp, _ := fs["mountPoint"].(map[interface{}]interface{})
		if mp == nil {
			continue
		}
		if path, _ := mp["path"].(string); path == "/" {
			rootMountID, _ = fs["deviceId"].(string)
			break
		}
	}

	if rootMountID == "" {
		tc.Fail("root mount point not found in host status filesystems")
		return nil
	}

	logrus.Infof("Root mount device ID: %s", rootMountID)

	// Find the verity device matching the root mount
	var verityDeviceName, dataDeviceID, hashDeviceID string
	verityList, _ := storage["verity"].([]interface{})
	for _, vRaw := range verityList {
		v, _ := vRaw.(map[interface{}]interface{})
		if v == nil {
			continue
		}
		if vID, _ := v["id"].(string); vID == rootMountID {
			verityDeviceName, _ = v["name"].(string)
			dataDeviceID, _ = v["dataDeviceId"].(string)
			hashDeviceID, _ = v["hashDeviceId"].(string)
			break
		}
	}

	if verityDeviceName == "" || hashDeviceID == "" {
		tc.Fail(fmt.Sprintf("no verity configuration found for root mount ID %q", rootMountID))
		return nil
	}

	logrus.Infof("Verity device: name=%s, dataDeviceId=%s, hashDeviceId=%s",
		verityDeviceName, dataDeviceID, hashDeviceID)

	// --- 5. Validate data/hash devices against block devices ---
	// After initial install, the active volume is always volume-a.
	abActiveVolume := "volume-a"

	_, hasABUpdate := storage["abUpdate"]
	if hasABUpdate {
		return s.validateVerityWithABUpdate(tc, blkidEntries, verityDeviceName,
			dataDeviceID, hashDeviceID, storage, abActiveVolume)
	}

	return s.validateVerityWithoutABUpdate(tc, blkidEntries, verityStatus,
		dataDeviceID, hashDeviceID)
}

// validateVerityWithABUpdate validates verity data/hash devices when A/B update
// is configured. It identifies the active volume pair members and checks they
// match the veritysetup output (resolving through RAID if needed).
func (s *TridentE2EScenario) validateVerityWithABUpdate(
	tc storm.TestCase,
	blkidEntries map[string]BlkidEntry,
	verityDeviceName, dataDeviceID, hashDeviceID string,
	storage map[interface{}]interface{},
	abActiveVolume string,
) error {
	// Find the active data and hash device IDs from A/B volume pairs
	var activeDataID, activeHashID string
	abUpdate, _ := storage["abUpdate"].(map[interface{}]interface{})
	volumePairs, _ := abUpdate["volumePairs"].([]interface{})

	for _, vpRaw := range volumePairs {
		vp, _ := vpRaw.(map[interface{}]interface{})
		if vp == nil {
			continue
		}
		vpID, _ := vp["id"].(string)

		if vpID == dataDeviceID {
			if abActiveVolume == "volume-a" {
				activeDataID, _ = vp["volumeAId"].(string)
			} else {
				activeDataID, _ = vp["volumeBId"].(string)
			}
		}

		if vpID == hashDeviceID {
			if abActiveVolume == "volume-a" {
				activeHashID, _ = vp["volumeAId"].(string)
			} else {
				activeHashID, _ = vp["volumeBId"].(string)
			}
		}
	}

	if activeDataID == "" || activeHashID == "" {
		tc.Fail(fmt.Sprintf(
			"could not find active A/B volume IDs for data=%q hash=%q (activeVolume=%s)",
			dataDeviceID, hashDeviceID, abActiveVolume))
		return nil
	}

	logrus.Infof("Active A/B volumes: data=%s, hash=%s", activeDataID, activeHashID)

	// Get data/hash block device paths from veritysetup status
	dataBlockDevice, hashBlockDevice, err := s.getVerityDevicePaths(verityDeviceName)
	if err != nil {
		return err
	}

	// Check if devices are RAID arrays
	dataRaidName, err := GetRaidNameFromDeviceName(s.sshClient, dataBlockDevice)
	if err != nil {
		return fmt.Errorf("failed to check RAID for data device %s: %w", dataBlockDevice, err)
	}

	hashRaidName, err := GetRaidNameFromDeviceName(s.sshClient, hashBlockDevice)
	if err != nil {
		return fmt.Errorf("failed to check RAID for hash device %s: %w", hashBlockDevice, err)
	}

	// Both must be the same type (both RAID or both partition)
	if (dataRaidName == "") != (hashRaidName == "") {
		tc.Fail(fmt.Sprintf(
			"data and hash devices must both be RAID or both be partitions: data_raid=%q, hash_raid=%q",
			dataRaidName, hashRaidName))
		return nil
	}

	if dataRaidName != "" {
		// RAID: extract name from path (e.g. /dev/md/root-a → root-a)
		extractedData := extractBaseName(dataRaidName)
		extractedHash := extractBaseName(hashRaidName)

		if extractedData != activeDataID {
			tc.Fail(fmt.Sprintf("expected active data RAID name %q, got %q",
				activeDataID, extractedData))
			return nil
		}

		if extractedHash != activeHashID {
			tc.Fail(fmt.Sprintf("expected active hash RAID name %q, got %q",
				activeHashID, extractedHash))
			return nil
		}
	} else {
		// Partition: look up PARTLABEL in blkid
		extractedData := extractBaseName(dataBlockDevice)
		extractedHash := extractBaseName(hashBlockDevice)

		dataEntry, dataOK := blkidEntries[extractedData]
		hashEntry, hashOK := blkidEntries[extractedHash]

		if !dataOK || !hashOK {
			tc.Fail(fmt.Sprintf(
				"data or hash block device not found in blkid: data=%q (found=%v), hash=%q (found=%v)",
				extractedData, dataOK, extractedHash, hashOK))
			return nil
		}

		dataPartLabel := dataEntry.Properties["PARTLABEL"]
		hashPartLabel := hashEntry.Properties["PARTLABEL"]

		if dataPartLabel != activeDataID {
			tc.Fail(fmt.Sprintf("expected data PARTLABEL %q, got %q",
				activeDataID, dataPartLabel))
			return nil
		}

		if hashPartLabel != activeHashID {
			tc.Fail(fmt.Sprintf("expected hash PARTLABEL %q, got %q",
				activeHashID, hashPartLabel))
			return nil
		}
	}

	logrus.Info("Verity A/B update device validation passed")
	return nil
}

// validateVerityWithoutABUpdate validates verity data/hash devices when no A/B
// update is configured. It checks the veritysetup status devices match the
// expected device IDs (resolving through RAID if needed).
func (s *TridentE2EScenario) validateVerityWithoutABUpdate(
	tc storm.TestCase,
	blkidEntries map[string]BlkidEntry,
	verityStatus VerityStatus,
	dataDeviceID, hashDeviceID string,
) error {
	dataBlockDevice := verityStatus.DataDevice
	hashBlockDevice := verityStatus.HashDevice

	// Check if devices are RAID arrays
	dataRaidName, err := GetRaidNameFromDeviceName(s.sshClient, dataBlockDevice)
	if err != nil {
		return fmt.Errorf("failed to check RAID for data device %s: %w", dataBlockDevice, err)
	}

	hashRaidName, err := GetRaidNameFromDeviceName(s.sshClient, hashBlockDevice)
	if err != nil {
		return fmt.Errorf("failed to check RAID for hash device %s: %w", hashBlockDevice, err)
	}

	// Both must be the same type
	if (dataRaidName == "") != (hashRaidName == "") {
		tc.Fail(fmt.Sprintf(
			"data and hash devices must both be RAID or both be partitions: data_raid=%q, hash_raid=%q",
			dataRaidName, hashRaidName))
		return nil
	}

	if dataRaidName != "" {
		// RAID: extract name (e.g. /dev/md/root → root)
		extractedData := extractBaseName(dataRaidName)
		extractedHash := extractBaseName(hashRaidName)

		if extractedData != dataDeviceID {
			tc.Fail(fmt.Sprintf("expected data RAID device ID %q, got %q",
				dataDeviceID, extractedData))
			return nil
		}

		if extractedHash != hashDeviceID {
			tc.Fail(fmt.Sprintf("expected hash RAID device ID %q, got %q",
				hashDeviceID, extractedHash))
			return nil
		}
	} else {
		// Partition: verify devices exist in blkid
		extractedData := extractBaseName(dataBlockDevice)
		extractedHash := extractBaseName(hashBlockDevice)

		if _, ok := blkidEntries[extractedData]; !ok {
			tc.Fail(fmt.Sprintf("data block device %q not found in blkid output",
				extractedData))
			return nil
		}

		if _, ok := blkidEntries[extractedHash]; !ok {
			tc.Fail(fmt.Sprintf("hash block device %q not found in blkid output",
				extractedHash))
			return nil
		}
	}

	logrus.Info("Verity device validation passed (no A/B update)")
	return nil
}

// getVerityDevicePaths runs `veritysetup status` for the given device name
// and extracts the data and hash block device paths.
func (s *TridentE2EScenario) getVerityDevicePaths(deviceName string) (string, string, error) {
	stdout, err := sudoCommand(s.sshClient,
		fmt.Sprintf("veritysetup status %s", deviceName))
	if err != nil {
		return "", "", fmt.Errorf("failed to run veritysetup status %s: %w",
			deviceName, err)
	}

	status := ParseVeritySetupStatus(stdout)

	if status.DataDevice == "" || status.HashDevice == "" {
		return "", "", fmt.Errorf(
			"failed to extract data/hash device from veritysetup status %s", deviceName)
	}

	return status.DataDevice, status.HashDevice, nil
}

// extractBaseName returns the last path component (e.g. "/dev/md/root-a" → "root-a",
// "/dev/sda3" → "sda3").
func extractBaseName(path string) string {
	if idx := strings.LastIndex(path, "/"); idx >= 0 {
		return path[idx+1:]
	}
	return path
}
