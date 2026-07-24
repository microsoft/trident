package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// MountEntry describes one line of `mount` output.
type MountEntry struct {
	Device     string
	MountPoint string
	FsType     string
}

// Mount runs `mount` on the host and returns the parsed entries.
func Mount(client *ssh.Client) ([]MountEntry, error) {
	out, err := sshutils.CommandOutput(client, "mount")
	if err != nil {
		return nil, fmt.Errorf("failed to run mount: %w", err)
	}
	return ParseMount(out), nil
}

// ParseMount parses `mount` output lines of the form
// "device on mount_point type fs_type (options)".
func ParseMount(stdout string) []MountEntry {
	var entries []MountEntry
	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 3 {
			continue
		}
		entry := MountEntry{
			Device:     fields[0],
			MountPoint: fields[2],
			FsType:     "unknown",
		}
		if len(fields) > 4 {
			entry.FsType = fields[4]
		}
		entries = append(entries, entry)
	}
	return entries
}

// RootDevice returns the device mounted at "/" and whether one was found.
func RootDevice(entries []MountEntry) (string, bool) {
	for _, e := range entries {
		if e.MountPoint == "/" {
			return e.Device, true
		}
	}
	return "", false
}
