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
	Name        string        `json:"name"`
	MajMin      string        `json:"maj:min"`
	RM          bool          `json:"rm"`
	Size        int64         `json:"size"`
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
func (o LsblkOutput) Partitions() []LsblkDevice {
	var partitions []LsblkDevice
	for _, bd := range o.Blockdevices {
		if len(bd.Children) == 0 {
			partitions = append(partitions, bd)
			continue
		}
		partitions = append(partitions, bd.Children...)
	}
	return partitions
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
