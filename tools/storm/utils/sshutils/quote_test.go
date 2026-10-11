package sshutils

import (
	"os/exec"
	"testing"
)

func TestShellQuoteNeutralizesShellMetacharacters(t *testing.T) {
	// Host Configuration permits these paths, and each one changes meaning if
	// it reaches the remote shell unquoted.
	tests := []struct{ name, in string }{
		{"spaces", "/var/lib/foo bar.raw"},
		{"variable expansion", "/tmp/$HOME/x.raw"},
		{"command substitution", "/tmp/`id`.raw"},
		{"single quote", "/tmp/it's.raw"},
		{"command chaining", "/tmp/x.raw; rm -rf /"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Round-trip through a real shell: the quoted word must arrive at
			// the command verbatim.
			out, err := exec.Command("sh", "-c", "printf %s "+ShellQuote(tt.in)).Output()
			if err != nil {
				t.Fatalf("shell rejected quoted word: %v", err)
			}
			if string(out) != tt.in {
				t.Errorf("shell received %q, want %q", string(out), tt.in)
			}
		})
	}
}
