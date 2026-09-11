package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// DmsetupInfo runs `sudo dmsetup info <name>` and returns the parsed key:value
// fields (Name, State, Tables present, UUID, ...).
func DmsetupInfo(client *ssh.Client, name string) (map[string]string, error) {
	out, err := sshutils.CommandOutput(client, fmt.Sprintf("sudo dmsetup info %s", name))
	if err != nil {
		return nil, fmt.Errorf("failed to run dmsetup info %s: %w", name, err)
	}
	return ParseKeyValueLines(out), nil
}

// ParseKeyValueLines parses lines of the form "key: value" into a map. Keys may
// contain spaces (e.g. "Tables present"); the split is on the first colon.
func ParseKeyValueLines(stdout string) map[string]string {
	result := make(map[string]string)
	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		key, value, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		key = strings.TrimSpace(key)
		value = strings.TrimSpace(value)
		if key != "" {
			result[key] = value
		}
	}
	return result
}
