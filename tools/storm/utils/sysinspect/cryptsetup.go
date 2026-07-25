package sysinspect

import (
	"encoding/json"
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// CryptsetupStatus is the parsed output of `cryptsetup status <name>`.
type CryptsetupStatus struct {
	// Active is true when the device is active (LUKS2 volumes are always open
	// and thus active).
	Active bool
	// InUse is true when the first line reports the device is "active and is in
	// use" (mounted); false when it is merely "active".
	InUse  bool
	Fields map[string]string
}

// Get returns a status field value and whether it was present.
func (s CryptsetupStatus) Get(field string) (string, bool) {
	v, ok := s.Fields[field]
	return v, ok
}

// Cryptsetup runs `sudo cryptsetup status <name>` and returns the parsed status.
func Cryptsetup(client *ssh.Client, name string) (CryptsetupStatus, error) {
	out, err := sshutils.CommandOutput(client, fmt.Sprintf("sudo cryptsetup status %s", name))
	if err != nil {
		return CryptsetupStatus{}, fmt.Errorf("failed to run cryptsetup status %s: %w", name, err)
	}
	return ParseCryptsetupStatus(out, name), nil
}

// ParseCryptsetupStatus parses `cryptsetup status <name>` output. The first line
// is the header ("<dev> is active[ and is in use]."); subsequent lines are
// "key: value" pairs.
func ParseCryptsetupStatus(stdout, name string) CryptsetupStatus {
	status := CryptsetupStatus{Fields: make(map[string]string)}

	lines := strings.Split(strings.TrimSpace(stdout), "\n")
	if len(lines) == 0 {
		return status
	}

	header := strings.TrimSpace(lines[0])
	inUseHeader := fmt.Sprintf("/dev/mapper/%s is active and is in use.", name)
	activeHeader := fmt.Sprintf("/dev/mapper/%s is active.", name)
	switch header {
	case inUseHeader:
		status.Active = true
		status.InUse = true
	case activeHeader:
		status.Active = true
	}

	for _, line := range lines[1:] {
		key, value, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		key = strings.TrimSpace(key)
		value = strings.TrimSpace(value)
		if key != "" {
			status.Fields[key] = value
		}
	}

	return status
}

// LuksDump is the subset of `cryptsetup luksDump --dump-json-metadata` output
// that the encryption validation inspects.
type LuksDump struct {
	Keyslots map[string]LuksKeyslot `json:"keyslots"`
	Tokens   map[string]LuksToken   `json:"tokens"`
	Digests  map[string]LuksDigest  `json:"digests"`
}

type LuksKeyslot struct {
	Type string `json:"type"`
	Kdf  struct {
		Type string `json:"type"`
		Hash string `json:"hash"`
	} `json:"kdf"`
	Area struct {
		Encryption string `json:"encryption"`
	} `json:"area"`
}

type LuksToken struct {
	Type        string   `json:"type"`
	Keyslots    []string `json:"keyslots"`
	Tpm2Pcrlock bool     `json:"tpm2_pcrlock"`
	Tpm2Pcrs    []int    `json:"tpm2-pcrs"`
}

type LuksDigest struct {
	Type string `json:"type"`
	Hash string `json:"hash"`
}

// CryptsetupLuksDump runs `cryptsetup luksDump --dump-json-metadata <path>` and
// returns the parsed metadata.
func CryptsetupLuksDump(client *ssh.Client, devicePath string) (LuksDump, error) {
	out, err := sshutils.CommandOutput(client,
		fmt.Sprintf("sudo cryptsetup luksDump --dump-json-metadata %s", devicePath))
	if err != nil {
		return LuksDump{}, fmt.Errorf("failed to run cryptsetup luksDump %s: %w", devicePath, err)
	}
	return ParseLuksDump(out)
}

// ParseLuksDump parses cryptsetup luksDump JSON metadata.
func ParseLuksDump(stdout string) (LuksDump, error) {
	var dump LuksDump
	if err := json.Unmarshal([]byte(stdout), &dump); err != nil {
		return LuksDump{}, fmt.Errorf("failed to parse luksDump JSON: %w", err)
	}
	return dump, nil
}
