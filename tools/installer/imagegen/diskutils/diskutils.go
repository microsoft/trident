// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Utility to create and manipulate disks and partitions

package diskutils

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strconv"
	"strings"

	"installer/internal/file"
	"installer/internal/shell"
)

// Unit to byte conversion values
const (
	B  = 1
	KB = 1000
	MB = 1000 * 1000
	GB = 1000 * 1000 * 1000
	TB = 1000 * 1000 * 1000 * 1000

	KiB = 1024
	MiB = 1024 * 1024
	GiB = 1024 * 1024 * 1024
	TiB = 1024 * 1024 * 1024 * 1024
)

// Boot type constants
const (
	EFIPartitionType    = "efi"
	LegacyPartitionType = "legacy"
)

var (
	sizeAndUnitRegexp = regexp.MustCompile(`(\d+)((Ki?|Mi?|Gi?|Ti?)?B)`)

	unitToBytes = map[string]uint64{
		"B":   B,
		"KB":  KB,
		"MB":  MB,
		"GB":  GB,
		"TB":  TB,
		"KiB": KiB,
		"MiB": MiB,
		"GiB": GiB,
		"TiB": TiB,
	}
)

type blockDevicesOutput struct {
	Devices []blockDeviceInfo `json:"blockdevices"`
}

type blockDeviceInfo struct {
	Name   string      `json:"name"`    // Example: sda
	MajMin string      `json:"maj:min"` // Example: 1:2
	Size   json.Number `json:"size"`    // Number of bytes. Can be a quoted string or a JSON number, depending on the util-linux version
	Model  string      `json:"model"`   // Example: 'Virtual Disk'
}

// SystemBlockDevice defines a block device on the host computer
type SystemBlockDevice struct {
	DevicePath  string // Example: /dev/sda
	RawDiskSize uint64 // Size in bytes
	Model       string // Example: Virtual Disk
}

// SystemBootType returns the current boot type of the system being ran on.
func SystemBootType() (bootType string) {
	// If a system booted with EFI, /sys/firmware/efi will exist
	const efiFirmwarePath = "/sys/firmware/efi"

	exist, _ := file.DirExists(efiFirmwarePath)
	if exist {
		bootType = EFIPartitionType
	} else {
		bootType = LegacyPartitionType
	}

	return
}

// BytesToSizeAndUnit takes a number of bytes and returns friendly representation of a size (for example 100GB).
func BytesToSizeAndUnit(bytes uint64) string {
	var (
		unitSize  uint64
		unitCount uint64
		unitName  string
	)

	sizes := []uint64{B, KiB, MiB, GiB, TiB}

	// Default to unit "Bytes" to handle the case where bytes is 0
	unitSize = B

	for _, unit := range sizes {
		if bytes >= unit {
			unitSize = unit
		}
	}

	for unit, unitBytes := range unitToBytes {
		if unitBytes == unitSize {
			unitName = unit
			break
		}
	}

	unitCount = bytes / unitSize

	return fmt.Sprintf("%d%s", unitCount, unitName)
}

// SizeAndUnitToBytes takes a friendly representation of a size (for example 100GB) and return the number of bytes it represents.
func SizeAndUnitToBytes(sizeAndUnit string) (bytes uint64, err error) {
	const (
		sizeIndex = 1
		unitIndex = 2
	)

	// Match size and unit.  Examples: 2GB, 512MiB
	matches := sizeAndUnitRegexp.FindAllStringSubmatch(sizeAndUnit, -1)

	// must be at least one match
	if len(matches) == 0 || len(matches[0]) <= 2 {
		err = fmt.Errorf("sizeAndUnit must contain a number and a unit type")
		return
	}
	match := matches[0]

	sizeString := match[sizeIndex]
	unit := match[unitIndex]

	size, err := strconv.ParseUint(sizeString, 10, 64)
	if err != nil {
		return
	}

	if unitBytes, ok := unitToBytes[unit]; ok {
		bytes = size * unitBytes
	} else {
		err = fmt.Errorf("unknown unit: %s", unit)
	}

	return
}

// SystemBlockDevices returns all block devices on the host system.
func SystemBlockDevices() (systemDevices []SystemBlockDevice, err error) {
	const (
		scsiDiskMajorNumber      = "8"
		mmcBlockMajorNumber      = "179"
		virtualDiskMajorNumber   = "252,253,254"
		blockExtendedMajorNumber = "259"
	)

	blockDeviceMajorNumbers := []string{scsiDiskMajorNumber, mmcBlockMajorNumber, virtualDiskMajorNumber, blockExtendedMajorNumber}
	includeFilter := strings.Join(blockDeviceMajorNumbers, ",")
	rawDiskOutput, stderr, err := shell.Execute("lsblk", "-d", "--bytes", "-I", includeFilter, "-n", "--json", "--output", "NAME,SIZE,MODEL")
	if err != nil {
		err = fmt.Errorf("failed to get disk list:\n%v\n%w", stderr, err)
		return
	}

	var blockDevices blockDevicesOutput
	if rawDiskOutput != "" {
		err = json.Unmarshal([]byte(rawDiskOutput), &blockDevices)
		if err != nil {
			err = fmt.Errorf("failed to parse disk list:\n%w", err)
			return
		}
	}

	if len(blockDevices.Devices) <= 0 {
		err = fmt.Errorf("no disks found")
		return
	}

	systemDevices = make([]SystemBlockDevice, len(blockDevices.Devices))

	for i, disk := range blockDevices.Devices {
		systemDevices[i].DevicePath = fmt.Sprintf("/dev/%s", disk.Name)
		systemDevices[i].Model = strings.TrimSpace(disk.Model)

		// Parse the size (which could be a quoted string or JSON number)
		size, err := disk.Size.Int64()
		if err != nil {
			err = fmt.Errorf("failed to parse disk size for %s: %w", disk.Name, err)
			return systemDevices, err
		}
		systemDevices[i].RawDiskSize = uint64(size)
	}

	return
}
