package sysinspect

import (
	"fmt"
	"regexp"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// mdSymlinkRe matches an `ls -l /dev/md` line, capturing the RAID array name
// and the md device it links to, e.g. "root-a -> ../md127".
var mdSymlinkRe = regexp.MustCompile(`(\S+)\s+->\s+\.\./(md\d+)`)

// RaidNameForDevice resolves a kernel md device path (e.g. "/dev/md127") to its
// friendly RAID array path (e.g. "/dev/md/root-a") by inspecting `ls -l /dev/md`.
// Returns ("", false) if /dev/md does not exist or no matching array is found.
//
// Mirrors base_test.py::get_raid_name_from_device_name.
func RaidNameForDevice(client *ssh.Client, deviceName string) (string, bool, error) {
	// Tolerate a missing /dev/md directory (non-RAID configs) without error.
	out, err := sshutils.RunCommand(client, "ls -l /dev/md")
	if err != nil {
		return "", false, fmt.Errorf("failed to run ls -l /dev/md: %w", err)
	}
	if out.Status != 0 {
		// Directory absent or empty: device is not a RAID array.
		return "", false, nil
	}

	name, found := parseRaidName(out.Stdout, deviceName)
	return name, found, nil
}

// parseRaidName extracts the friendly RAID path for the given md device from
// `ls -l /dev/md` output. deviceName may be a full path ("/dev/md127") or bare
// name ("md127").
func parseRaidName(stdout, deviceName string) (string, bool) {
	mdName := deviceName[strings.LastIndex(deviceName, "/")+1:]

	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		matches := mdSymlinkRe.FindStringSubmatch(line)
		if matches == nil {
			continue
		}
		// matches[1] = array name, matches[2] = md device (e.g. md127)
		if matches[2] == mdName {
			return "/dev/md/" + matches[1], true
		}
	}
	return "", false
}
