package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// Users runs `cat /etc/passwd` and returns the set of usernames on the host.
func Users(client *ssh.Client) (map[string]struct{}, error) {
	out, err := sshutils.CommandOutput(client, "cat /etc/passwd")
	if err != nil {
		return nil, fmt.Errorf("failed to read /etc/passwd: %w", err)
	}
	return ParsePasswd(out), nil
}

// ParsePasswd parses /etc/passwd content into a set of usernames (the first
// colon-separated field of each line).
func ParsePasswd(stdout string) map[string]struct{} {
	users := make(map[string]struct{})
	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		name, _, _ := strings.Cut(line, ":")
		users[name] = struct{}{}
	}
	return users
}

// Groups runs `cat /etc/group` and returns, for each group, the set of member
// usernames listed in the final field.
func Groups(client *ssh.Client) (map[string]map[string]struct{}, error) {
	out, err := sshutils.CommandOutput(client, "cat /etc/group")
	if err != nil {
		return nil, fmt.Errorf("failed to read /etc/group: %w", err)
	}
	return ParseGroup(out), nil
}

// ParseGroup parses /etc/group content into a map of group name -> member set.
// Lines look like "wheel:x:10:testing-user,other".
func ParseGroup(stdout string) map[string]map[string]struct{} {
	groups := make(map[string]map[string]struct{})
	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		parts := strings.Split(line, ":")
		if len(parts) < 1 {
			continue
		}
		members := make(map[string]struct{})
		if len(parts) >= 4 && parts[len(parts)-1] != "" {
			for _, m := range strings.Split(parts[len(parts)-1], ",") {
				if m != "" {
					members[m] = struct{}{}
				}
			}
		}
		groups[parts[0]] = members
	}
	return groups
}
