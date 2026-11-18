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

const (
	// mdDirectory holds the friendly-name symlinks mdadm creates per array.
	mdDirectory = "/dev/md"
	// noMdDirectorySentinel distinguishes "this host has no arrays" from an
	// inspection failure, which `ls` reports with the same exit status.
	noMdDirectorySentinel = "NO_MD_DIRECTORY"
)

// RaidNameForDevice resolves a kernel md device path (e.g. "/dev/md127") to its
// friendly RAID array path (e.g. "/dev/md/root-a") by inspecting `ls -l /dev/md`.
// Returns ("", false) if /dev/md does not exist or no matching array is found.
//
// Mirrors base_test.py::get_raid_name_from_device_name.
func RaidNameForDevice(client *ssh.Client, deviceName string) (string, bool, error) {
	// A non-RAID host has no /dev/md at all, which must not be an error. But
	// `ls` exits 2 for BOTH a missing directory and an unreadable one, so the
	// exit status cannot tell them apart: treating every non-zero status as
	// "not a RAID device" silently downgrades an inspection failure into a
	// confident negative, and the callers then validate the wrong device set.
	// Probe for the directory explicitly so only a genuine absence is benign.
	out, err := sshutils.RunCommand(client,
		fmt.Sprintf("if [ ! -d %[1]s ]; then echo %[2]s; else ls -l %[1]s; fi", mdDirectory, noMdDirectorySentinel))
	if err != nil {
		return "", false, fmt.Errorf("failed to inspect %s: %w", mdDirectory, err)
	}
	if out.Status != 0 {
		return "", false, fmt.Errorf("failed to inspect %s: exit status %d: %s",
			mdDirectory, out.Status, strings.TrimSpace(out.Stderr))
	}
	if strings.TrimSpace(out.Stdout) == noMdDirectorySentinel {
		// No /dev/md: this host has no software RAID arrays.
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
