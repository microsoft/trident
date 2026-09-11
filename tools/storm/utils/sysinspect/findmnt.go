package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// FindmntRow is one row of `findmnt <target>` output.
type FindmntRow struct {
	Target  string
	Source  string
	FsType  string
	Options string
}

// Findmnt runs `sudo findmnt <target>` and returns the parsed rows.
func Findmnt(client *ssh.Client, target string) ([]FindmntRow, error) {
	out, err := sshutils.CommandOutput(client, fmt.Sprintf("sudo findmnt %s", target))
	if err != nil {
		return nil, fmt.Errorf("failed to run findmnt %s: %w", target, err)
	}
	return ParseFindmnt(out), nil
}

// ParseFindmnt parses `findmnt <target>` table output. The first line is the
// header (TARGET SOURCE FSTYPE OPTIONS); columns are whitespace-separated.
func ParseFindmnt(stdout string) []FindmntRow {
	lines := strings.Split(strings.TrimSpace(stdout), "\n")
	if len(lines) < 2 {
		return nil
	}

	var rows []FindmntRow
	for _, line := range lines[1:] {
		fields := strings.Fields(line)
		if len(fields) < 3 {
			continue
		}
		row := FindmntRow{
			Target: fields[0],
			Source: fields[1],
			FsType: fields[2],
		}
		if len(fields) > 3 {
			row.Options = fields[3]
		}
		rows = append(rows, row)
	}
	return rows
}
