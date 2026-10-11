package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// ActiveSwaps returns the set of active swap device paths, canonicalized via
// `readlink -f`. Mirrors encryption_test.py::get_active_swaps.
func ActiveSwaps(client *ssh.Client) (map[string]struct{}, error) {
	// Capture swapon's output before canonicalizing it. Piping straight into
	// xargs takes the pipeline's status from xargs, which succeeds on empty
	// input, so a swapon failure would surface as "no swaps are active" and
	// the encryption validator would report a missing swap rather than the
	// inspection error.
	cmd := "out=$(swapon --show=NAME --raw --bytes --noheadings) || exit $?; " +
		"printf '%s\n' \"$out\" | xargs -r -I @ readlink -f @"
	out, err := sshutils.CommandOutput(client, cmd)
	if err != nil {
		return nil, fmt.Errorf("failed to list active swaps: %w", err)
	}

	swaps := make(map[string]struct{})
	for _, line := range strings.Split(strings.TrimSpace(out), "\n") {
		line = strings.TrimSpace(line)
		if line != "" {
			swaps[line] = struct{}{}
		}
	}
	return swaps, nil
}

// ReadlinkF resolves a path to its canonical absolute form via `readlink -f`.
func ReadlinkF(client *ssh.Client, path string) (string, error) {
	out, err := sshutils.CommandOutput(client, fmt.Sprintf("sudo readlink -f %s", sshutils.ShellQuote(path)))
	if err != nil {
		return "", fmt.Errorf("failed to readlink -f %s: %w", path, err)
	}
	return strings.TrimSpace(out), nil
}

// Getenforce returns the current SELinux enforcement mode ("Enforcing",
// "Permissive", or "Disabled").
func Getenforce(client *ssh.Client) (string, error) {
	out, err := sshutils.CommandOutput(client, "sudo getenforce")
	if err != nil {
		return "", fmt.Errorf("failed to run getenforce: %w", err)
	}
	return strings.TrimSpace(out), nil
}

// Setenforce sets the SELinux enforcement mode (0 = permissive, 1 = enforcing).
func Setenforce(client *ssh.Client, enforcing bool) error {
	mode := "0"
	if enforcing {
		mode = "1"
	}
	_, err := sshutils.CommandOutput(client, "sudo setenforce "+mode)
	if err != nil {
		return fmt.Errorf("failed to run setenforce %s: %w", mode, err)
	}
	return nil
}
