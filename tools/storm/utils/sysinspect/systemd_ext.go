package sysinspect

import (
	"encoding/json"
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// systemdExtHierarchy is one entry of `systemd-sysext status --json` /
// `systemd-confext status --json` output: a hierarchy (e.g. /usr, /opt, /etc)
// with the list of extensions currently merged into it.
type systemdExtHierarchy struct {
	Hierarchy  string        `json:"hierarchy"`
	Extensions stringOrSlice `json:"extensions"`
}

// stringOrSlice decodes a JSON value that systemd emits either as an array of
// strings, a single bare string (e.g. "none" when a hierarchy has no
// extensions merged), or null. It normalizes all three into a []string.
type stringOrSlice []string

func (s *stringOrSlice) UnmarshalJSON(data []byte) error {
	trimmed := strings.TrimSpace(string(data))
	if trimmed == "null" {
		*s = nil
		return nil
	}
	// Array form: ["a","b"].
	if strings.HasPrefix(trimmed, "[") {
		var list []string
		if err := json.Unmarshal(data, &list); err != nil {
			return err
		}
		*s = list
		return nil
	}
	// Scalar string form: "none" / "some-ext".
	var single string
	if err := json.Unmarshal(data, &single); err != nil {
		return err
	}
	*s = []string{single}
	return nil
}

// SystemdExtStatus runs `systemd-<extType> status --json=pretty` on the host and
// returns the set of active extension names across all hierarchies. extType is
// "sysext" or "confext".
func SystemdExtStatus(client *ssh.Client, extType string) (map[string]struct{}, error) {
	cmd := fmt.Sprintf("sudo systemd-%s status --json=pretty --no-pager", extType)
	out, err := sshutils.RunCommand(client, cmd)
	if err != nil {
		return nil, fmt.Errorf("failed to run systemd-%s status: %w", extType, err)
	}
	if err := out.Check(); err != nil {
		return nil, fmt.Errorf("systemd-%s status failed: %s", extType, out.Report())
	}
	return ParseSystemdExtStatus(out.Stdout)
}

// ParseSystemdExtStatus parses `systemd-sysext/confext status --json` output
// into the set of active extension names. Separated for unit testing.
func ParseSystemdExtStatus(stdout string) (map[string]struct{}, error) {
	var hierarchies []systemdExtHierarchy
	if err := json.Unmarshal([]byte(stdout), &hierarchies); err != nil {
		return nil, fmt.Errorf("failed to parse systemd extension status JSON: %w", err)
	}

	active := make(map[string]struct{})
	for _, h := range hierarchies {
		for _, ext := range h.Extensions {
			// systemd reports "none" (as a bare string) for a hierarchy with no
			// extensions merged; it is a sentinel, not an extension name.
			if strings.EqualFold(ext, "none") {
				continue
			}
			active[ext] = struct{}{}
		}
	}
	return active, nil
}
