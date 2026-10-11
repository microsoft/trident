package sysinspect

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// EfiBootInfo is the parsed result of `efibootmgr`.
type EfiBootInfo struct {
	// BootCurrent is the active boot entry number (e.g. "0001").
	BootCurrent string
	// EntryNames maps a boot entry number (e.g. "0001") to its label (the token
	// following "Boot####*" on the entry line).
	EntryNames map[string]string
}

// CurrentName returns the label of the currently-booted entry and whether both
// BootCurrent and its label were found.
func (e EfiBootInfo) CurrentName() (string, bool) {
	if e.BootCurrent == "" {
		return "", false
	}
	name, ok := e.EntryNames[e.BootCurrent]
	return name, ok
}

// EfiBootMgr runs `sudo efibootmgr` and returns the parsed boot info.
func EfiBootMgr(client *ssh.Client) (EfiBootInfo, error) {
	out, err := sshutils.CommandOutput(client, "sudo efibootmgr")
	if err != nil {
		return EfiBootInfo{}, fmt.Errorf("failed to run efibootmgr: %w", err)
	}
	return ParseEfiBootMgr(out), nil
}

// ParseEfiBootMgr parses `efibootmgr` output, extracting BootCurrent and the
// label of each Boot#### entry. Mirrors base_test.py::test_uefi_fallback.
//
// Example:
//
//	BootCurrent: 0001
//	BootOrder: 0001,0000
//	Boot0000* UiApp
//	Boot0001* azl	HD(1,GPT,...)
func ParseEfiBootMgr(stdout string) EfiBootInfo {
	info := EfiBootInfo{EntryNames: make(map[string]string)}

	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		line = strings.TrimSpace(line)
		switch {
		case strings.HasPrefix(line, "BootCurrent:"):
			_, value, _ := strings.Cut(line, ":")
			info.BootCurrent = strings.TrimSpace(value)

		case strings.HasPrefix(line, "Boot"):
			// Entry lines look like "Boot0001* label ...". Skip non-entry
			// "Boot*" lines such as BootOrder/BootNext.
			fields := strings.Fields(line)
			if len(fields) < 2 {
				continue
			}
			// fields[0] = "Boot0001*" (or "Boot0001"); extract the 4-hex number.
			head := strings.TrimPrefix(fields[0], "Boot")
			head = strings.TrimSuffix(head, "*")
			if len(head) != 4 {
				continue
			}
			info.EntryNames[head] = fields[1]
		}
	}

	return info
}
