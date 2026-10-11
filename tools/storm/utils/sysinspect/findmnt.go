package sysinspect

import (
	"encoding/json"
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// FindmntRow is one row of `findmnt <target>` output.
type FindmntRow struct {
	Target  string `json:"target"`
	Source  string `json:"source"`
	FsType  string `json:"fstype"`
	Options string `json:"options"`
}

// findmntOutput is the shape of `findmnt --json`.
type findmntOutput struct {
	Filesystems []FindmntRow `json:"filesystems"`
}

// Findmnt runs `sudo findmnt --json <target>` and returns the parsed rows.
//
// JSON rather than the default table because a mount point may contain
// whitespace: the table separates columns with spaces and does not escape them
// there, so splitting on whitespace silently shifts every field and reports a
// mismatch against a perfectly valid configured mount point.
func Findmnt(client *ssh.Client, target string) ([]FindmntRow, error) {
	out, err := sshutils.CommandOutput(client,
		fmt.Sprintf("sudo findmnt --json %s", sshutils.ShellQuote(target)))
	if err != nil {
		return nil, fmt.Errorf("failed to run findmnt %s: %w", target, err)
	}
	return ParseFindmnt(out)
}

// ParseFindmnt parses `findmnt --json` output. An empty result is not an
// error: findmnt prints nothing and exits non-zero when the target is not
// mounted, and callers distinguish that themselves.
func ParseFindmnt(stdout string) ([]FindmntRow, error) {
	if strings.TrimSpace(stdout) == "" {
		return nil, nil
	}
	var parsed findmntOutput
	if err := json.Unmarshal([]byte(stdout), &parsed); err != nil {
		return nil, fmt.Errorf("failed to parse findmnt output: %w", err)
	}
	return parsed.Filesystems, nil
}
