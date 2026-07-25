// Package sysinspect provides parsers that gather structured host state from a
// running system over SSH. Each helper runs a standard inspection tool (blkid,
// lsblk, mount, cryptsetup, veritysetup, ...) and returns typed data for E2E
// validations. These replace the ad-hoc string parsing previously done in the
// Python E2E suite (tests/e2e_tests/*.py).
package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// BlkidEntry holds the tag=value fields blkid reports for a single block
// device, e.g. UUID, TYPE, PARTLABEL, PARTUUID, LABEL, BLOCK_SIZE.
type BlkidEntry struct {
	// Device is the kernel device name (e.g. "sda1"), i.e. the basename of the
	// device path reported by blkid.
	Device string
	// Path is the full device path reported by blkid (e.g. "/dev/sda1",
	// "/dev/mapper/root").
	Path string
	// Fields holds the parsed tag=value pairs (quotes stripped).
	Fields map[string]string
}

// Get returns the value of a blkid field and whether it was present.
func (e BlkidEntry) Get(field string) (string, bool) {
	v, ok := e.Fields[field]
	return v, ok
}

// Blkid runs `sudo blkid` on the host and returns the parsed entries keyed by
// kernel device name (basename of the device path).
//
// Example line:
//
//	/dev/sda2: UUID="04267584-..." BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="root-a" PARTUUID="f1be3a27-..."
func Blkid(client *ssh.Client) (map[string]BlkidEntry, error) {
	out, err := sshutils.CommandOutput(client, "sudo blkid")
	if err != nil {
		return nil, fmt.Errorf("failed to run blkid: %w", err)
	}
	return ParseBlkid(out), nil
}

// ParseBlkid parses the stdout of `blkid` into entries keyed by kernel device
// name. It is separated from Blkid so it can be unit-tested without SSH.
func ParseBlkid(stdout string) map[string]BlkidEntry {
	entries := make(map[string]BlkidEntry)

	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		// Split "device: field=val field=val ..." into device and the rest.
		devicePart, rest, found := strings.Cut(line, ": ")
		if !found {
			continue
		}

		device := devicePart[strings.LastIndex(devicePart, "/")+1:]
		entry := BlkidEntry{Device: device, Path: devicePart, Fields: make(map[string]string)}

		for _, field := range strings.Fields(rest) {
			key, value, ok := strings.Cut(field, "=")
			if !ok {
				continue
			}
			entry.Fields[key] = strings.Trim(value, "\"")
		}

		entries[device] = entry
	}

	return entries
}
