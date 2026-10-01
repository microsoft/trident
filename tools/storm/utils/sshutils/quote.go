package sshutils

import "strings"

// ShellQuote renders s as a single POSIX shell word.
//
// Commands sent over SSH are interpreted by a remote shell, and Host
// Configuration permits mount points and image paths containing spaces and
// shell metacharacters. Go's %q is not a shell quoter -- it leaves `$` and
// backticks live -- so paths interpolated into a remote command must go
// through this instead.
func ShellQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}
