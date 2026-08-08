package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// VeritySetupStatus is the parsed output of `veritysetup status <name>`.
type VeritySetupStatus struct {
	// Active is true when the first status line reports the device is active
	// and in use.
	Active bool
	// Fields holds the "key: value" lines below the header (e.g. "type",
	// "status", "mode", "data device", "hash device").
	Fields map[string]string
}

// Get returns a status field value and whether it was present.
func (s VeritySetupStatus) Get(field string) (string, bool) {
	v, ok := s.Fields[field]
	return v, ok
}

// VeritySetup runs `sudo veritysetup status <name>` on the host and returns the
// parsed status.
func VeritySetup(client *ssh.Client, name string) (VeritySetupStatus, error) {
	out, err := sshutils.CommandOutput(client, fmt.Sprintf("sudo veritysetup status %s", name))
	if err != nil {
		return VeritySetupStatus{}, fmt.Errorf("failed to run veritysetup status %s: %w", name, err)
	}
	return ParseVeritySetupStatus(out, name), nil
}

// ParseVeritySetupStatus parses `veritysetup status <name>` output. The first
// line is the "<dev> is active and is in use." header; subsequent lines are
// "key: value" pairs (keys may contain spaces, e.g. "data device").
func ParseVeritySetupStatus(stdout, name string) VeritySetupStatus {
	status := VeritySetupStatus{Fields: make(map[string]string)}

	lines := strings.Split(strings.TrimSpace(stdout), "\n")
	if len(lines) == 0 {
		return status
	}

	header := strings.TrimSpace(lines[0])
	status.Active = header == fmt.Sprintf("/dev/mapper/%s is active and is in use.", name)

	for _, line := range lines[1:] {
		key, value, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		key = strings.TrimSpace(key)
		value = strings.TrimSpace(value)
		if key != "" && value != "" {
			status.Fields[key] = value
		}
	}

	return status
}
