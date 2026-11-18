package trident

import (
	"errors"
	"testing"

	"golang.org/x/crypto/ssh"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
)

func TestCheckCommandResult(t *testing.T) {
	for _, tt := range []struct {
		name          string
		out           *stormsshconfig.SshCmdOutput
		err           error
		reboot, valid bool
	}{
		{"success", &stormsshconfig.SshCmdOutput{}, nil, false, true},
		{"failure", &stormsshconfig.SshCmdOutput{Status: 1}, nil, false, false},
		{"missing output", nil, nil, false, false},
		{"unannounced disconnect", &stormsshconfig.SshCmdOutput{Status: -1}, &ssh.ExitMissingError{}, true, false},
		{"unexpected disconnect", &stormsshconfig.SshCmdOutput{Stderr: REBOOTING_LOG_MESSAGE, Status: -1}, &ssh.ExitMissingError{}, false, false},
		{"announced reboot", &stormsshconfig.SshCmdOutput{Stderr: REBOOTING_LOG_MESSAGE, Status: -1}, &ssh.ExitMissingError{}, true, true},
		{"other transport failure", &stormsshconfig.SshCmdOutput{Stderr: REBOOTING_LOG_MESSAGE, Status: -1}, errors.New("connection failed"), true, false},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if err := CheckCommandResult(tt.out, tt.err, tt.reboot); (err == nil) != tt.valid {
				t.Fatalf("valid=%v, got %v", tt.valid, err)
			}
		})
	}
}
