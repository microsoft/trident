package scenario

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"unicode"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/utils/trident"
)

// parseSizeToBytes converts a partition size string (e.g. "8G", "512M", "1024")
// to bytes, matching the Python SizeUnit enum from base_test.py.
func parseSizeToBytes(sizeStr string) (float64, error) {
	if sizeStr == "" {
		return 0, fmt.Errorf("empty size string")
	}

	lastChar := rune(sizeStr[len(sizeStr)-1])
	if unicode.IsLetter(lastChar) {
		numberStr := sizeStr[:len(sizeStr)-1]
		number, err := strconv.ParseFloat(numberStr, 64)
		if err != nil {
			return 0, fmt.Errorf("failed to parse size number %q: %w", numberStr, err)
		}

		unitMultipliers := map[rune]float64{
			'B': 1,
			'K': math.Pow(1024, 1),
			'M': math.Pow(1024, 2),
			'G': math.Pow(1024, 3),
			'T': math.Pow(1024, 4),
			'P': math.Pow(1024, 5),
		}

		multiplier, ok := unitMultipliers[unicode.ToUpper(lastChar)]
		if !ok {
			return 0, fmt.Errorf("unknown size unit %q", string(lastChar))
		}

		return number * multiplier, nil
	}

	return strconv.ParseFloat(sizeStr, 64)
}

// validatePartitions validates that disk partitions on the remote host match
// the expected host configuration. Converted from base_test.py test_partitions.
//
// It runs blkid, lsblk, mount, and trident get on the remote host, then checks:
//   - Each expected partition (by PARTLABEL) is present in system info and host status
//   - servicingState is "provisioned"
//   - For A/B update configs: validates root mount is on the correct active volume
func (s *TridentE2EScenario) validatePartitions(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	// --- 1. Build expected partitions from host configuration ---
	expectedPartitions := make(map[string]float64)

	for _, disk := range s.originalConfig.S("storage", "disks").Children() {
		for _, part := range disk.S("partitions").Children() {
			id, ok := part.S("id").Data().(string)
			if !ok {
				continue
			}

			sizeStr, _ := part.S("size").Data().(string)
			sizeBytes, err := parseSizeToBytes(sizeStr)
			if err != nil {
				logrus.WithError(err).Warnf("Failed to parse size for partition %s", id)
				sizeBytes = 0
			}

			expectedPartitions[id] = sizeBytes
		}
	}

	logrus.Infof("Expected %d partitions from host configuration", len(expectedPartitions))

	// --- 2. Run blkid and parse output ---
	blkidOut, err := sudoCommand(s.sshClient, "blkid")
	if err != nil {
		return fmt.Errorf("failed to run blkid: %w", err)
	}

	blkidEntries := ParseBlkid(blkidOut)

	// --- 3. Run lsblk -J -b and parse output ---
	lsblkOut, err := runCommand(s.sshClient, "lsblk -J -b")
	if err != nil {
		return fmt.Errorf("failed to run lsblk: %w", err)
	}

	lsblkData, err := ParseLsblk(lsblkOut)
	if err != nil {
		return fmt.Errorf("failed to parse lsblk output: %w", err)
	}

	// --- 4. Merge lsblk info into blkid entries, then index by PARTLABEL ---
	for _, lsblkPart := range lsblkData.FlattenPartitions() {
		if _, exists := blkidEntries[lsblkPart.Name]; !exists {
			blkidEntries[lsblkPart.Name] = BlkidEntry{
				Properties: make(map[string]string),
			}
		}

		entry := blkidEntries[lsblkPart.Name]
		entry.Properties["lsblk_size"] = lsblkPart.Size.String()
		entry.Properties["lsblk_name"] = lsblkPart.Name
		entry.Properties["lsblk_type"] = lsblkPart.Type
		blkidEntries[lsblkPart.Name] = entry
	}

	partitionsByLabel := make(map[string]BlkidEntry)
	for _, entry := range blkidEntries {
		if label, ok := entry.Properties["PARTLABEL"]; ok {
			partitionsByLabel[label] = entry
		}
	}

	// --- 5. Get host status via trident get ---
	tridentOut, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "get")
	if err != nil {
		return fmt.Errorf("failed to run trident get: %w", err)
	}

	if tridentOut.Status != 0 {
		return fmt.Errorf("trident get failed with status %d: %s", tridentOut.Status, tridentOut.Stderr)
	}

	hostStatus, err := ParseTridentGetOutput(tridentOut.Stdout)
	if err != nil {
		return fmt.Errorf("failed to parse trident get output: %w", err)
	}

	// --- 6. Check servicing state ---
	servicingState, _ := hostStatus["servicingState"].(string)
	if servicingState != "provisioned" {
		tc.Fail(fmt.Sprintf("expected servicingState 'provisioned', got %q", servicingState))
		return nil
	}

	// --- 7. Check that each expected partition is present ---
	partitionPaths, _ := hostStatus["partitionPaths"].(map[interface{}]interface{})
	for partID := range expectedPartitions {
		if _, ok := partitionPaths[partID]; !ok {
			tc.Fail(fmt.Sprintf("partition %q not found in host status partitionPaths", partID))
			return nil
		}

		if _, ok := partitionsByLabel[partID]; !ok {
			tc.Fail(fmt.Sprintf("partition %q not found in system partition info (by PARTLABEL)", partID))
			return nil
		}
	}

	// --- 8. Find root device from mount ---
	mountOut, err := runCommand(s.sshClient, "mount")
	if err != nil {
		return fmt.Errorf("failed to run mount: %w", err)
	}

	mountEntries := ParseMount(mountOut)
	rootDevicePath := FindRootDevice(mountEntries)

	// --- 9. A/B update validation ---
	spec, _ := hostStatus["spec"].(map[interface{}]interface{})
	if spec == nil {
		logrus.Info("Partition validation passed (no spec in host status)")
		return nil
	}

	storage, _ := spec["storage"].(map[interface{}]interface{})
	if storage == nil {
		logrus.Info("Partition validation passed (no storage in spec)")
		return nil
	}

	if _, hasABUpdate := storage["abUpdate"]; !hasABUpdate {
		logrus.Info("Partition validation passed (no A/B update configured)")
		return nil
	}

	// After initial install, the active volume is always volume-a.
	abActiveVolume := "volume-a"

	// Find root mount ID from filesystems
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

	logrus.Infof("Root mount point ID: %s", rootMountID)

	// Check for verity device on root mount
	var verityDeviceName, verityDataDeviceID string
	verityList, _ := storage["verity"].([]interface{})
	for _, vRaw := range verityList {
		v, _ := vRaw.(map[interface{}]interface{})
		if v == nil {
			continue
		}
		if vID, _ := v["id"].(string); vID == rootMountID {
			verityDeviceName, _ = v["name"].(string)
			verityDataDeviceID, _ = v["dataDeviceId"].(string)
			logrus.Infof("Found verity device with matching ID %q", rootMountID)
			break
		}
	}

	logrus.Infof("Verity device name: %s, data device ID: %s", verityDeviceName, verityDataDeviceID)

	// Determine A/B volume ID: if verity, use the data device; otherwise, use root mount
	abVolumeID := rootMountID
	if verityDataDeviceID != "" {
		abVolumeID = verityDataDeviceID
	}

	logrus.Infof("Root A/B volume ID: %s", abVolumeID)

	// For non-verity configs, validate the active volume block device path
	if verityDeviceName == "" {
		var activeVolumeID string
		abUpdate, _ := storage["abUpdate"].(map[interface{}]interface{})
		volumePairs, _ := abUpdate["volumePairs"].([]interface{})

		for _, vpRaw := range volumePairs {
			vp, _ := vpRaw.(map[interface{}]interface{})
			if vp == nil {
				continue
			}
			if vpID, _ := vp["id"].(string); vpID == abVolumeID {
				logrus.Infof("Found volume pair: %s", abVolumeID)
				if abActiveVolume == "volume-a" {
					activeVolumeID, _ = vp["volumeAId"].(string)
				} else {
					activeVolumeID, _ = vp["volumeBId"].(string)
				}
				logrus.Infof("Active volume ID: %s", activeVolumeID)
				break
			}
		}

		if activeVolumeID == "" {
			tc.Fail("active volume ID not found for root A/B volume pair")
			return nil
		}

		activeIsPartition := IsPartition(hostStatus, activeVolumeID)
		activeIsRaid := IsRaid(hostStatus, activeVolumeID)
		if activeIsPartition == activeIsRaid {
			tc.Fail(fmt.Sprintf("active volume %q must be either a partition or RAID (not both/neither): partition=%v, raid=%v",
				activeVolumeID, activeIsPartition, activeIsRaid))
			return nil
		}

		// Resolve the expected root device path
		var expectedRootPath string

		if activeIsPartition {
			canonicalName := rootDevicePath
			if idx := strings.LastIndex(rootDevicePath, "/"); idx >= 0 {
				canonicalName = rootDevicePath[idx+1:]
			}
			if entry, ok := blkidEntries[canonicalName]; ok {
				if partuuid, ok := entry.Properties["PARTUUID"]; ok {
					expectedRootPath = fmt.Sprintf("/dev/disk/by-partuuid/%s", partuuid)
				}
			}
		} else if activeIsRaid {
			raidName, err := GetRaidNameFromDeviceName(s.sshClient, rootDevicePath)
			if err != nil {
				return fmt.Errorf("failed to get RAID name for %q: %w", rootDevicePath, err)
			}
			expectedRootPath = raidName
		}

		// Verify that the active volume path matches in partitionPaths
		for bdevID, bdevPathRaw := range partitionPaths {
			bdevIDStr, _ := bdevID.(string)
			if bdevIDStr == activeVolumeID {
				bdevPath, _ := bdevPathRaw.(string)
				if bdevPath != expectedRootPath {
					tc.Fail(fmt.Sprintf("active volume path mismatch for %q: expected %q, got %q",
						activeVolumeID, expectedRootPath, bdevPath))
					return nil
				}
			}
		}

		// Verify abActiveVolume from host status
		hsActiveVolume, _ := hostStatus["abActiveVolume"].(string)
		if hsActiveVolume != abActiveVolume {
			tc.Fail(fmt.Sprintf("abActiveVolume mismatch: expected %q, got %q", abActiveVolume, hsActiveVolume))
			return nil
		}
	}

	logrus.Info("Partition validation passed")
	return nil
}
