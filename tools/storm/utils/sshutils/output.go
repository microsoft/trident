package sshutils

import (
	"fmt"
	"strings"
)

type SshCmdOutput struct {
	Stdout string
	Stderr string
	Status int
}

// Returns an error if the command finished with a non-zero status.
func (o *SshCmdOutput) Check() error {
	if o.Status != 0 {
		return fmt.Errorf("command failed with status %d", o.Status)
	}

	return nil
}

func (o *SshCmdOutput) Report() string {
	if o == nil {
		return "<nil>"
	}

	var stringBuilder strings.Builder
	stringBuilder.WriteString(fmt.Sprintf("status: %d", o.Status))

	if o.Stdout != "" {
		stringBuilder.WriteString(fmt.Sprintf("; stdout:\n%s\nstderr:", o.Stdout))
	} else {
		stringBuilder.WriteString("; stdout: <empty>; stderr:")
	}

	if o.Stderr != "" {
		stringBuilder.WriteString(fmt.Sprintf("\n%s", o.Stderr))
	} else {
		stringBuilder.WriteString(" <empty>")
	}

	return stringBuilder.String()
}
