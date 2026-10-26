package validate

import (
	"math"
	"strconv"
	"strings"

	"tridenttools/pkg/hostconfig"
	tridentutil "tridenttools/storm/utils/trident"
)

// sizeUnits maps a single-letter size suffix to its multiplier (powers of 1024),
// matching base_test.py's SizeUnit enum.
var sizeUnits = map[byte]float64{
	'B': 1,
	'K': math.Pow(1024, 1),
	'M': math.Pow(1024, 2),
	'G': math.Pow(1024, 3),
	'T': math.Pow(1024, 4),
	'P': math.Pow(1024, 5),
}

// ParseSizeToBytes converts a Host Configuration partition size string (e.g.
// "8G", "192M", "1024") into bytes. The final character may be a unit suffix
// (B/K/M/G/T/P); a bare number is treated as bytes. Non-numeric sizes such as
// "grow" return ok=false so callers can skip the size expectation.
func ParseSizeToBytes(size string) (int64, bool) {
	size = strings.TrimSpace(size)
	if size == "" {
		return 0, false
	}

	last := size[len(size)-1]
	numPart := size
	multiplier := 1.0

	if unit, isUnit := sizeUnits[last]; isUnit {
		multiplier = unit
		numPart = size[:len(size)-1]
	} else if last < '0' || last > '9' {
		// Trailing non-digit, non-unit character (e.g. "grow"): not a size.
		return 0, false
	}

	value, err := strconv.ParseFloat(numPart, 64)
	if err != nil {
		return 0, false
	}

	return int64(value * multiplier), true
}

// PartitionExpectation is a partition declared in the Host Configuration.
type PartitionExpectation struct {
	ID        string
	SizeBytes int64
	HasSize   bool
}

// ExpectedPartitions extracts the partitions declared across all disks in the
// given Host Configuration spec.
func ExpectedPartitions(spec hostconfig.HostConfig) []PartitionExpectation {
	var result []PartitionExpectation
	for _, disk := range spec.S("storage", "disks").Children() {
		for _, part := range disk.S("partitions").Children() {
			id, ok := part.S("id").Data().(string)
			if !ok {
				continue
			}
			exp := PartitionExpectation{ID: id}
			if sizeStr, ok := part.S("size").Data().(string); ok {
				exp.SizeBytes, exp.HasSize = ParseSizeToBytes(sizeStr)
			}
			result = append(result, exp)
		}
	}
	return result
}

// IsPartition reports whether the block device ID corresponds to a disk
// partition declared in the spec.
func IsPartition(spec hostconfig.HostConfig, deviceID string) bool {
	for _, disk := range spec.S("storage", "disks").Children() {
		for _, part := range disk.S("partitions").Children() {
			if id, ok := part.S("id").Data().(string); ok && id == deviceID {
				return true
			}
		}
	}
	return false
}

// IsRaid reports whether the block device ID corresponds to a software RAID
// array declared in the spec.
func IsRaid(spec hostconfig.HostConfig, deviceID string) bool {
	for _, raid := range spec.S("storage", "raid", "software").Children() {
		if id, ok := raid.S("id").Data().(string); ok && id == deviceID {
			return true
		}
	}
	return false
}

// mountPointPath extracts the "/" path from a filesystem's mountPoint field,
// which may be either a plain string or an object with a "path" key. Returns
// ("", false) if no mount point is set.
func mountPointPath(fs *hostconfig.HostConfig) (string, bool) {
	mp := fs.S("mountPoint")
	if mp == nil || mp.Data() == nil {
		return "", false
	}
	if str, ok := mp.Data().(string); ok {
		return str, true
	}
	if path, ok := mp.S("path").Data().(string); ok {
		return path, true
	}
	return "", false
}

// RootFilesystemDeviceID returns the deviceId of the filesystem mounted at "/"
// in the spec, and whether one was found.
func RootFilesystemDeviceID(spec hostconfig.HostConfig) (string, bool) {
	for _, fs := range spec.S("storage", "filesystems").Children() {
		path, ok := mountPointPath(&hostconfig.HostConfig{Container: fs})
		if !ok || path != "/" {
			continue
		}
		if id, ok := fs.S("deviceId").Data().(string); ok {
			return id, true
		}
	}
	return "", false
}

// ActiveVolumeID resolves the active volume's block-device ID for the A/B
// volume pair whose `id` equals volumePairID, given the active A/B selection.
// Returns ("", false) if no matching volume pair exists.
func ActiveVolumeID(spec hostconfig.HostConfig, volumePairID string, active tridentutil.AbVolumeSelection) (string, bool) {
	for _, pair := range spec.S("storage", "abUpdate", "volumePairs").Children() {
		id, ok := pair.S("id").Data().(string)
		if !ok || id != volumePairID {
			continue
		}
		field := "volumeAId"
		if active == tridentutil.AbVolumeB {
			field = "volumeBId"
		}
		if vid, ok := pair.S(field).Data().(string); ok {
			return vid, true
		}
	}
	return "", false
}

// verityDataDeviceID returns the dataDeviceId of the verity device whose id
// matches the given device ID, and whether such a verity device exists.
func verityDataDeviceID(spec hostconfig.HostConfig, deviceID string) (string, bool) {
	for _, v := range spec.S("storage", "verity").Children() {
		if id, ok := v.S("id").Data().(string); ok && id == deviceID {
			if data, ok := v.S("dataDeviceId").Data().(string); ok {
				return data, true
			}
			return "", true
		}
	}
	return "", false
}

// AbVolumePairID returns the A/B volume-pair ID backing the root filesystem.
// If root sits on a verity device, the pair ID is the verity data device;
// otherwise it is the root filesystem's device ID directly. The second return
// value is true when root is a verity device.
func AbVolumePairID(spec hostconfig.HostConfig, rootDeviceID string) (pairID string, rootIsVerity bool) {
	if dataID, isVerity := verityDataDeviceID(spec, rootDeviceID); isVerity {
		return dataID, true
	}
	return rootDeviceID, false
}
