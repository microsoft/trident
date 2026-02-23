package scenario

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"tridenttools/storm/utils/sshutils"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v2"
)

// --- SSH command helpers ---

// sudoCommand runs a command with sudo on the remote host and returns the
// trimmed stdout. Returns an error if the command fails with a non-zero exit.
func sudoCommand(client *ssh.Client, cmd string) (string, error) {
	fullCmd := fmt.Sprintf("sudo %s", cmd)
	logrus.WithField("command", fullCmd).Debug("Running remote command")

	out, err := sshutils.RunCommand(client, fullCmd)
	if err != nil {
		return "", fmt.Errorf("failed to run command %q: %w", fullCmd, err)
	}

	if err := out.Check(); err != nil {
		return "", fmt.Errorf("command %q failed (status %d): %s\nstderr: %s",
			fullCmd, out.Status, err, out.Stderr)
	}

	return strings.TrimSpace(out.Stdout), nil
}

// runCommand runs a command (without sudo) on the remote host and returns the
// trimmed stdout. Returns an error if the command fails with a non-zero exit.
func runCommand(client *ssh.Client, cmd string) (string, error) {
	logrus.WithField("command", cmd).Debug("Running remote command")

	out, err := sshutils.RunCommand(client, cmd)
	if err != nil {
		return "", fmt.Errorf("failed to run command %q: %w", cmd, err)
	}

	if err := out.Check(); err != nil {
		return "", fmt.Errorf("command %q failed (status %d): %s\nstderr: %s",
			cmd, out.Status, err, out.Stderr)
	}

	return strings.TrimSpace(out.Stdout), nil
}

// --- blkid parser ---

// BlkidEntry holds parsed properties for a single block device from blkid output.
type BlkidEntry struct {
	DevicePath string
	Properties map[string]string
}

// ParseBlkid parses the output of `blkid` (standard format, one device per line).
// Example line:
//
//	/dev/sda1: UUID="D920-8BA4" BLOCK_SIZE="512" TYPE="vfat" PARTLABEL="esp" PARTUUID="6fcc..."
//
// Returns a map keyed by the short device name (e.g. "sda1") to BlkidEntry.
func ParseBlkid(stdout string) map[string]BlkidEntry {
	entries := make(map[string]BlkidEntry)

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		parts := strings.SplitN(line, ": ", 2)
		if len(parts) < 1 {
			continue
		}

		devPath := parts[0]
		// Extract short name: /dev/sda1 → sda1, /dev/mapper/root → root
		shortName := devPath
		if idx := strings.LastIndex(devPath, "/"); idx >= 0 {
			shortName = devPath[idx+1:]
		}

		props := make(map[string]string)
		if len(parts) == 2 {
			for _, token := range splitBlkidFields(parts[1]) {
				kv := strings.SplitN(token, "=", 2)
				if len(kv) == 2 {
					props[kv[0]] = strings.Trim(kv[1], "\"")
				}
			}
		}

		entries[shortName] = BlkidEntry{
			DevicePath: devPath,
			Properties: props,
		}
	}

	return entries
}

// splitBlkidFields splits a blkid property string respecting quoted values.
func splitBlkidFields(s string) []string {
	var fields []string
	var current strings.Builder
	inQuote := false

	for _, ch := range s {
		switch {
		case ch == '"':
			inQuote = !inQuote
			current.WriteRune(ch)
		case ch == ' ' && !inQuote:
			if current.Len() > 0 {
				fields = append(fields, current.String())
				current.Reset()
			}
		default:
			current.WriteRune(ch)
		}
	}
	if current.Len() > 0 {
		fields = append(fields, current.String())
	}

	return fields
}

// ParseBlkidExport parses the output of `blkid --output export`.
// Returns a map keyed by DEVNAME (e.g. "/dev/md127") to properties.
func ParseBlkidExport(stdout string) map[string]map[string]string {
	devs := make(map[string]map[string]string)
	var currentDev string

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			currentDev = ""
			continue
		}

		kv := strings.SplitN(line, "=", 2)
		if len(kv) != 2 {
			continue
		}

		if kv[0] == "DEVNAME" {
			currentDev = kv[1]
			devs[currentDev] = make(map[string]string)
		} else if currentDev != "" {
			devs[currentDev][kv[0]] = kv[1]
		}
	}

	return devs
}

// --- lsblk JSON parser ---

// LsblkOutput represents the top-level JSON from `lsblk -J -b`.
type LsblkOutput struct {
	BlockDevices []LsblkDevice `json:"blockdevices"`
}

// LsblkDevice represents a block device in lsblk JSON output.
type LsblkDevice struct {
	Name        string        `json:"name"`
	MajMin      string        `json:"maj:min"`
	Rm          bool          `json:"rm"`
	Size        json.Number   `json:"size"`
	Ro          bool          `json:"ro"`
	Type        string        `json:"type"`
	MountPoints []interface{} `json:"mountpoints"`
	Children    []LsblkDevice `json:"children,omitempty"`
}

// ParseLsblk parses JSON output from `lsblk -J -b` and returns the structure.
func ParseLsblk(stdout string) (*LsblkOutput, error) {
	var output LsblkOutput
	if err := json.Unmarshal([]byte(stdout), &output); err != nil {
		return nil, fmt.Errorf("failed to parse lsblk JSON: %w", err)
	}
	return &output, nil
}

// FlattenPartitions returns all leaf-level partitions across all block devices.
// Block devices with no children are treated as partitions themselves.
func (o *LsblkOutput) FlattenPartitions() []LsblkDevice {
	var partitions []LsblkDevice
	for _, bd := range o.BlockDevices {
		if len(bd.Children) == 0 {
			partitions = append(partitions, bd)
		} else {
			partitions = append(partitions, bd.Children...)
		}
	}
	return partitions
}

// --- mount output parser ---

// MountEntry represents a single mount point from `mount` output.
type MountEntry struct {
	Device     string
	MountPoint string
	FsType     string
	Options    string
}

// ParseMount parses the output of the `mount` command.
// Each line has format: <device> on <mountpoint> type <fstype> (<options>)
func ParseMount(stdout string) []MountEntry {
	var entries []MountEntry

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		parts := strings.Fields(line)
		if len(parts) < 5 {
			continue
		}

		entry := MountEntry{
			Device:     parts[0],
			MountPoint: parts[2],
			FsType:     parts[4],
		}

		if len(parts) > 5 {
			entry.Options = strings.Trim(parts[5], "()")
		}

		entries = append(entries, entry)
	}

	return entries
}

// FindRootDevice returns the device path mounted at "/".
func FindRootDevice(entries []MountEntry) string {
	for _, e := range entries {
		if e.MountPoint == "/" {
			return e.Device
		}
	}
	return ""
}

// --- /etc/passwd parser ---

// PasswdEntry represents a single line from /etc/passwd.
type PasswdEntry struct {
	Username string
	UID      string
	GID      string
	Home     string
	Shell    string
}

// ParsePasswd parses /etc/passwd content and returns entries keyed by username.
func ParsePasswd(stdout string) map[string]PasswdEntry {
	entries := make(map[string]PasswdEntry)

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		fields := strings.Split(line, ":")
		if len(fields) < 7 {
			continue
		}

		entries[fields[0]] = PasswdEntry{
			Username: fields[0],
			UID:      fields[2],
			GID:      fields[3],
			Home:     fields[5],
			Shell:    fields[6],
		}
	}

	return entries
}

// --- /etc/group parser ---

// GroupEntry represents a single line from /etc/group.
type GroupEntry struct {
	Name    string
	GID     string
	Members []string
}

// ParseGroup parses /etc/group content and returns entries keyed by group name.
func ParseGroup(stdout string) map[string]GroupEntry {
	entries := make(map[string]GroupEntry)

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		fields := strings.Split(line, ":")
		if len(fields) < 4 {
			continue
		}

		var members []string
		if fields[3] != "" {
			members = strings.Split(fields[3], ",")
		}

		entries[fields[0]] = GroupEntry{
			Name:    fields[0],
			GID:     fields[2],
			Members: members,
		}
	}

	return entries
}

// --- efibootmgr parser ---

// EfiBootInfo holds parsed efibootmgr output.
type EfiBootInfo struct {
	BootCurrent string
	BootEntries map[string]string // Boot number → entry name/description
}

// ParseEfiBootMgr parses efibootmgr output.
// Example:
//
//	BootCurrent: 0001
//	Boot0000* EFI DVD/CDROM
//	Boot0001* Azure Linux
func ParseEfiBootMgr(stdout string) EfiBootInfo {
	info := EfiBootInfo{
		BootEntries: make(map[string]string),
	}

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)

		if strings.HasPrefix(line, "BootCurrent:") {
			info.BootCurrent = strings.TrimSpace(strings.SplitN(line, ":", 2)[1])
			continue
		}

		if strings.HasPrefix(line, "Boot") && len(line) > 8 && line[8] == '*' {
			// Pattern: Boot0001* Azure Linux
			numStr := line[4:8]
			rest := strings.TrimSpace(line[9:])
			info.BootEntries[numStr] = rest
		}
	}

	return info
}

// CurrentBootName returns the name/description of the current boot entry.
func (e *EfiBootInfo) CurrentBootName() string {
	if e.BootCurrent == "" {
		return ""
	}

	// Look up the current boot entry, return just the first word (name)
	if desc, ok := e.BootEntries[e.BootCurrent]; ok {
		fields := strings.Fields(desc)
		if len(fields) > 0 {
			return fields[0]
		}
		return desc
	}

	return ""
}

// --- Key-value line parser (cryptsetup status, veritysetup status, dmsetup info) ---

// ParseKeyValueLines parses lines in "key: value" format into a map.
func ParseKeyValueLines(stdout string) map[string]string {
	result := make(map[string]string)

	for _, line := range strings.Split(stdout, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		kv := strings.SplitN(line, ":", 2)
		if len(kv) == 2 {
			result[strings.TrimSpace(kv[0])] = strings.TrimSpace(kv[1])
		}
	}

	return result
}

// --- Table parser (findmnt, swapon) ---

// ParseTable parses whitespace-separated table output with a header row.
// Returns a slice of maps, one per data row.
func ParseTable(stdout string) []map[string]string {
	lines := strings.Split(strings.TrimSpace(stdout), "\n")
	if len(lines) < 2 {
		return nil
	}

	headers := strings.Fields(lines[0])
	var rows []map[string]string

	for _, line := range lines[1:] {
		fields := strings.Fields(line)
		row := make(map[string]string)
		for i, h := range headers {
			if i < len(fields) {
				row[h] = fields[i]
			}
		}
		rows = append(rows, row)
	}

	return rows
}

// --- YAML parser for trident get output ---

// ParseTridentGetOutput parses the YAML output of `trident get` into a
// generic map structure. Handles YAML tags (e.g. !image) gracefully.
func ParseTridentGetOutput(stdout string) (map[string]interface{}, error) {
	var result map[string]interface{}
	if err := yaml.Unmarshal([]byte(stdout), &result); err != nil {
		return nil, fmt.Errorf("failed to parse trident get YAML: %w", err)
	}
	return result, nil
}

// --- RAID name resolver ---

var raidNameRegex = regexp.MustCompile(`(\S+)\s+->\s+\.\./(\S+)`)

// ParseDevMdListing parses the output of `ls -l /dev/md` and returns a map
// from md device number (e.g. "md127") to RAID name path (e.g. "/dev/md/root-a").
func ParseDevMdListing(stdout string) map[string]string {
	result := make(map[string]string)

	for _, line := range strings.Split(stdout, "\n") {
		matches := raidNameRegex.FindStringSubmatch(line)
		if len(matches) == 3 {
			raidName := matches[1]
			mdDevice := matches[2]
			result[mdDevice] = fmt.Sprintf("/dev/md/%s", raidName)
		}
	}

	return result
}

// GetRaidNameFromDeviceName resolves a device name like "/dev/md127" to its
// RAID name like "/dev/md/root-a" by parsing `ls -l /dev/md`.
func GetRaidNameFromDeviceName(client *ssh.Client, deviceName string) (string, error) {
	stdout, err := sudoCommand(client, "ls -l /dev/md || true")
	if err != nil {
		return "", nil // non-fatal, /dev/md may not exist
	}

	if strings.Contains(stdout, "No such file or directory") || stdout == "" {
		return "", nil
	}

	// Extract the md device number: /dev/md127 → md127
	mdDeviceNumber := deviceName
	if idx := strings.LastIndex(deviceName, "/"); idx >= 0 {
		mdDeviceNumber = deviceName[idx+1:]
	}

	raidMap := ParseDevMdListing(stdout)
	if name, ok := raidMap[mdDeviceNumber]; ok {
		return name, nil
	}

	return "", nil
}

// --- cryptsetup status parser ---

// CryptsetupStatus holds parsed output from `cryptsetup status <name>`.
type CryptsetupStatus struct {
	Type        string
	Cipher      string
	Keysize     string
	KeyLocation string
	Device      string
	SectorSize  string
	Offset      string
	Size        string
	Mode        string
	Properties  map[string]string
}

// ParseCryptsetupStatus parses the output of `cryptsetup status <name>`.
func ParseCryptsetupStatus(stdout string) CryptsetupStatus {
	kv := ParseKeyValueLines(stdout)
	status := CryptsetupStatus{
		Type:        kv["type"],
		Cipher:      kv["cipher"],
		Keysize:     kv["keysize"],
		KeyLocation: kv["key location"],
		Device:      kv["device"],
		SectorSize:  kv["sector size"],
		Offset:      kv["offset"],
		Size:        kv["size"],
		Mode:        kv["mode"],
		Properties:  kv,
	}
	return status
}

// --- dmsetup info parser ---

// DmsetupInfo holds parsed output from `dmsetup info <name>`.
type DmsetupInfo struct {
	Name           string
	State          string
	ReadAhead      string
	TablesPresent  string
	OpenCount      string
	EventNumber    string
	MajorMinor     string
	NumberOfTargets string
	UUID           string
	Properties     map[string]string
}

// ParseDmsetupInfo parses the output of `dmsetup info <name>`.
func ParseDmsetupInfo(stdout string) DmsetupInfo {
	kv := ParseKeyValueLines(stdout)
	info := DmsetupInfo{
		Name:           kv["Name"],
		State:          kv["State"],
		ReadAhead:      kv["Read Ahead"],
		TablesPresent:  kv["Tables present"],
		OpenCount:      kv["Open count"],
		EventNumber:    kv["Event number"],
		MajorMinor:     kv["Major, minor"],
		NumberOfTargets: kv["Number of targets"],
		UUID:           kv["UUID"],
		Properties:     kv,
	}
	return info
}

// --- veritysetup status parser ---

// VerityStatus holds parsed veritysetup status output.
type VerityStatus struct {
	IsActive     bool
	IsInUse      bool
	Properties   map[string]string
	DataDevice   string
	HashDevice   string
	StatusLine   string
}

// ParseVeritySetupStatus parses the output of `veritysetup status <name>`.
func ParseVeritySetupStatus(stdout string) VerityStatus {
	lines := strings.Split(strings.TrimSpace(stdout), "\n")
	status := VerityStatus{
		Properties: make(map[string]string),
	}

	if len(lines) == 0 {
		return status
	}

	status.StatusLine = lines[0]
	status.IsActive = strings.Contains(lines[0], "is active")
	status.IsInUse = strings.Contains(lines[0], "is in use")

	for _, line := range lines[1:] {
		kv := strings.SplitN(strings.TrimSpace(line), ":", 2)
		if len(kv) == 2 {
			key := strings.TrimSpace(kv[0])
			val := strings.TrimSpace(kv[1])
			status.Properties[key] = val

			switch key {
			case "data device":
				status.DataDevice = val
			case "hash device":
				status.HashDevice = val
			}
		}
	}

	return status
}

// --- cryptsetup luksDump JSON parser ---

// LuksDump represents the JSON metadata from `cryptsetup luksDump --dump-json-metadata`.
type LuksDump struct {
	Keyslots map[string]LuksKeyslot `json:"keyslots"`
	Tokens   map[string]LuksToken   `json:"tokens"`
	Segments map[string]LuksSegment `json:"segments"`
	Digests  map[string]LuksDigest  `json:"digests"`
	Config   LuksConfig             `json:"config"`
}

// LuksKeyslot represents a LUKS2 keyslot.
type LuksKeyslot struct {
	Type    string      `json:"type"`
	KeySize int         `json:"key_size"`
	KDF     LuksKDF     `json:"kdf"`
	Area    LuksArea    `json:"area"`
	AF      interface{} `json:"af"`
}

// LuksKDF represents the Key Derivation Function config.
type LuksKDF struct {
	Type       string `json:"type"`
	Hash       string `json:"hash"`
	Iterations int    `json:"iterations"`
	Salt       string `json:"salt"`
}

// LuksArea represents the keyslot area config.
type LuksArea struct {
	Type       string `json:"type"`
	Encryption string `json:"encryption"`
	KeySize    int    `json:"key_size"`
}

// LuksToken represents a LUKS2 token.
type LuksToken struct {
	Type       string   `json:"type"`
	Keyslots   []string `json:"keyslots"`
	TPM2PCRs   []int    `json:"tpm2-pcrs"`
	TPM2PCRLock *bool   `json:"tpm2_pcrlock,omitempty"`
}

// LuksSegment represents a LUKS2 segment.
type LuksSegment struct {
	Type       string `json:"type"`
	Encryption string `json:"encryption"`
	SectorSize int    `json:"sector_size"`
}

// LuksDigest represents a LUKS2 digest.
type LuksDigest struct {
	Type string `json:"type"`
	Hash string `json:"hash"`
}

// LuksConfig represents the LUKS2 config section.
type LuksConfig struct {
	JsonSize     string `json:"json_size"`
	KeyslotsSize string `json:"keyslots_size"`
}

// ParseLuksDump parses JSON output from `cryptsetup luksDump --dump-json-metadata`.
func ParseLuksDump(stdout string) (*LuksDump, error) {
	var dump LuksDump
	if err := json.Unmarshal([]byte(stdout), &dump); err != nil {
		return nil, fmt.Errorf("failed to parse luksDump JSON: %w", err)
	}
	return &dump, nil
}

// --- systemd-sysext/confext status parser ---

// SysextHierarchy represents one hierarchy entry from systemd-sysext status JSON.
type SysextHierarchy struct {
	Hierarchy  string   `json:"hierarchy"`
	Extensions []string `json:"extensions"`
}

// ParseSysextStatus parses JSON output from `systemd-sysext status --json=pretty`.
func ParseSysextStatus(stdout string) ([]SysextHierarchy, error) {
	var hierarchies []SysextHierarchy
	if err := json.Unmarshal([]byte(stdout), &hierarchies); err != nil {
		return nil, fmt.Errorf("failed to parse sysext status JSON: %w", err)
	}
	return hierarchies, nil
}

// AllActiveExtensions returns a flat list of all active extension names across hierarchies.
func AllActiveExtensions(hierarchies []SysextHierarchy) []string {
	var exts []string
	for _, h := range hierarchies {
		exts = append(exts, h.Extensions...)
	}
	return exts
}

// --- Host config helpers ---

// IsPartition checks if a device ID refers to a disk partition in the host status.
func IsPartition(hostStatus map[string]interface{}, blockDeviceID string) bool {
	spec, ok := hostStatus["spec"].(map[interface{}]interface{})
	if !ok {
		return false
	}
	storage, ok := spec["storage"].(map[interface{}]interface{})
	if !ok {
		return false
	}
	disks, ok := storage["disks"].([]interface{})
	if !ok {
		return false
	}

	for _, d := range disks {
		disk, ok := d.(map[interface{}]interface{})
		if !ok {
			continue
		}
		partitions, ok := disk["partitions"].([]interface{})
		if !ok {
			continue
		}
		for _, p := range partitions {
			part, ok := p.(map[interface{}]interface{})
			if !ok {
				continue
			}
			if id, ok := part["id"].(string); ok && id == blockDeviceID {
				return true
			}
		}
	}

	return false
}

// IsRaid checks if a device ID refers to a software RAID array in the host status.
func IsRaid(hostStatus map[string]interface{}, blockDeviceID string) bool {
	spec, ok := hostStatus["spec"].(map[interface{}]interface{})
	if !ok {
		return false
	}
	storage, ok := spec["storage"].(map[interface{}]interface{})
	if !ok {
		return false
	}
	raid, ok := storage["raid"].(map[interface{}]interface{})
	if !ok {
		return false
	}
	software, ok := raid["software"].([]interface{})
	if !ok {
		return false
	}

	for _, r := range software {
		arr, ok := r.(map[interface{}]interface{})
		if !ok {
			continue
		}
		if id, ok := arr["id"].(string); ok && id == blockDeviceID {
			return true
		}
	}

	return false
}

// CheckPathExists verifies that a path exists on the remote host.
func CheckPathExists(client *ssh.Client, path string) error {
	_, err := sudoCommand(client, fmt.Sprintf("ls %s", path))
	return err
}
