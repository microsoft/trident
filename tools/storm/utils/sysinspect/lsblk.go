package sysinspect

import (
	"encoding/json"
	"fmt"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// LsblkDevice is a single node in the `lsblk -J` tree. Sizes are in bytes when
// lsblk is invoked with -b.
type LsblkDevice struct {
	Name   string `json:"name"`
	MajMin string `json:"maj:min"`
	RM     bool   `json:"rm"`
	// json.Number because util-linux emits this as either a quoted string or a
	// bare number depending on its version, and an int64 here fails to decode
	// the string form -- which would fail the whole lsblk parse, not just this
	// field. Same reasoning as tools/installer/imagegen/diskutils.
	Size        json.Number   `json:"size"`
	RO          bool          `json:"ro"`
	Type        string        `json:"type"`
	Mountpoints []*string     `json:"mountpoints"`
	Children    []LsblkDevice `json:"children,omitempty"`
}

// LsblkOutput is the top-level structure of `lsblk -J` output.
type LsblkOutput struct {
	Blockdevices []LsblkDevice `json:"blockdevices"`
}

// Partitions flattens the block-device tree into the set of leaf devices,
// treating any block device without children as a partition (mirrors the
// flattening done in base_test.py).
//
// Descends the whole tree rather than one level: lsblk nests device-mapper and
// LVM nodes beneath a partition, so stopping at the first level would return
// the parent and omit the actual leaf.
func (o LsblkOutput) Partitions() []LsblkDevice {
	var partitions []LsblkDevice
	var walk func(devices []LsblkDevice)
	walk = func(devices []LsblkDevice) {
		for _, bd := range devices {
			if len(bd.Children) == 0 {
				partitions = append(partitions, bd)
				continue
			}
			walk(bd.Children)
		}
	}
	walk(o.Blockdevices)
	return partitions
}

// FindDevice returns the device with the given kernel name from anywhere in the
// tree. Partition sizes are checked by joining the Host Status partition paths
// (e.g. /dev/sda3) to the tree, and the match can sit at any depth.
func (o LsblkOutput) FindDevice(name string) (LsblkDevice, bool) {
	var found LsblkDevice
	var ok bool
	var walk func(devices []LsblkDevice)
	walk = func(devices []LsblkDevice) {
		for _, bd := range devices {
			if ok {
				return
			}
			if bd.Name == name {
				found, ok = bd, true
				return
			}
			walk(bd.Children)
		}
	}
	walk(o.Blockdevices)
	return found, ok
}

// Lsblk runs `lsblk -J -b` on the host and returns the parsed tree with sizes
// in bytes.
func Lsblk(client *ssh.Client) (LsblkOutput, error) {
	out, err := sshutils.CommandOutput(client, "lsblk -J -b")
	if err != nil {
		return LsblkOutput{}, fmt.Errorf("failed to run lsblk: %w", err)
	}
	return ParseLsblk(out)
}

// ParseLsblk parses `lsblk -J -b` JSON output. Separated from Lsblk for unit
// testing without SSH.
func ParseLsblk(stdout string) (LsblkOutput, error) {
	var parsed LsblkOutput
	if err := json.Unmarshal([]byte(stdout), &parsed); err != nil {
		return LsblkOutput{}, fmt.Errorf("failed to parse lsblk JSON: %w", err)
	}
	return parsed, nil
}
