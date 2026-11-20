package acl

import (
	"encoding/json"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strings"

	"github.com/google/uuid"
	"gopkg.in/yaml.v3"
	stormactivevolume "tridenttools/storm/utils/trident/activevolume"
)

type fileDigest struct {
	SHA256 string
	Size   int64
}

type snapshot struct {
	FSType                                                         string
	BootID, Kernel, OSRelease                                      string
	Verity, Source, UsrDevice, Options, Ancestors, Bootctl, Status string
	MSHVPresent, MSHVCharacter                                     bool
	Trees                                                          map[string]map[string]fileDigest
}

func validateSnapshot(current, reference snapshot, expected checkpoint, info artifactInfo) error {
	if expected.Slot != "a" && expected.Slot != "b" {
		return fmt.Errorf("invalid expected slot %q", expected.Slot)
	}
	partition, slot, other := "sda2", "AZLA", "AZLB"
	if expected.Slot == "b" {
		partition, slot, other = "sda4", "AZLB", "AZLA"
	}
	id, err := uuid.Parse(strings.TrimSpace(current.BootID))
	if err != nil || id == uuid.Nil {
		return fmt.Errorf("invalid boot ID %q", current.BootID)
	}
	if !slices.Contains(strings.Fields(current.Ancestors), partition) {
		return fmt.Errorf("/usr is not backed by expected slot partition %s", partition)
	}
	if current.Source == "" || current.Source != current.UsrDevice || current.FSType != "btrfs" || !slices.Contains(strings.Split(current.Options, ","), "ro") || !strings.Contains(strings.ToLower(current.Verity), "verity") {
		return fmt.Errorf("/usr must be read-only and verity-backed")
	}
	if !slices.Contains(strings.Split(current.OSRelease, "\n"), "VARIANT_ID=azurecontainerlinux") {
		return fmt.Errorf("expected native ACL os-release")
	}
	if info.Variant == variantMSHV {
		if !strings.Contains(current.Kernel, "mshv") || !current.MSHVPresent || !current.MSHVCharacter {
			return fmt.Errorf("MSHV kernel and character device /dev/mshv required")
		}
	} else if strings.Contains(current.Kernel, "mshv") || current.MSHVPresent {
		return fmt.Errorf("regular kernel must not expose MSHV")
	}
	if expected.Phase == "baseline" {
		if len(current.Trees["BOOT"]) == 0 {
			return fmt.Errorf("baseline fallback boot tree is empty")
		}
		return nil
	}
	if err := stormactivevolume.ValidateHostStatusState(current.Status, "volume-"+expected.Slot, expected.State); err != nil {
		return err
	}
	var status map[string]any
	if err := yaml.Unmarshal([]byte(current.Status), &status); err != nil {
		return err
	}
	if _, exists := status["lastError"]; exists {
		return fmt.Errorf("Trident status contains lastError: %v", status["lastError"])
	}
	referenceSlot := slot
	if expected.Previous == "baseline" {
		referenceSlot = "BOOT"
	}
	if len(current.Trees[slot]) == 0 || !maps.Equal(current.Trees[slot], reference.Trees[referenceSlot]) {
		return fmt.Errorf("%s boot tree changed unexpectedly", slot)
	}
	if strings.HasSuffix(expected.Phase, "-staged") {
		if current.BootID != reference.BootID || !maps.Equal(current.Trees["BOOT"], reference.Trees["BOOT"]) {
			return fmt.Errorf("staging changed the running boot or fallback tree")
		}
		if len(current.Trees[other]) == 0 {
			return fmt.Errorf("inactive %s boot tree missing", other)
		}
		if expected.State == "manual-rollback-ab-staged" && !maps.Equal(current.Trees[other], reference.Trees[other]) {
			return fmt.Errorf("rollback staging modified retained %s", other)
		}
		return nil
	}
	if !maps.Equal(current.Trees[other], reference.Trees[other]) || !maps.Equal(current.Trees["BOOT"], current.Trees[slot]) {
		return fmt.Errorf("inactive tree changed or committed fallback does not match %s", slot)
	}
	bootFile := `[^/]+\.efi`
	if info.Variant == variantMSHV {
		bootFile = `second\.efi`
		for _, name := range []string{"bootx64.efi", "grubx64.efi", "second.efi", "HvLoader.efi", "lxhvloader.dll", "Windows/System32/hvix64.exe", "Windows/System32/hvax64.exe", "Windows/System32/kdstub.dll"} {
			if current.Trees[slot][strings.ToLower(name)].Size <= 0 {
				return fmt.Errorf("%s/%s missing or empty", slot, name)
			}
		}
	} else if _, exists := current.Trees[slot]["hvloader.efi"]; exists {
		return fmt.Errorf("standard slot unexpectedly contains HvLoader")
	}
	if !regexp.MustCompile(`(?im)^\s*File: .*EFI/` + slot + `/` + bootFile + `\s*$`).MatchString(current.Bootctl) {
		return fmt.Errorf("bootctl does not identify %s loader", slot)
	}
	return nil
}

func (r *runner) check(expected checkpoint, info artifactInfo) error {
	out := snapshot{Trees: make(map[string]map[string]fileDigest)}
	measurements := []struct {
		name, command string
		target        *string
	}{
		{"boot-id", "cat /proc/sys/kernel/random/boot_id", &out.BootID},
		{"cmdline", "cat /proc/cmdline", nil},
		{"os-release", "cat /usr/lib/os-release", &out.OSRelease},
		{"kernel", "uname -r", &out.Kernel},
		{"usr-source", `readlink -f "$(findmnt -rn -o SOURCE --nofsroot /usr)"`, &out.Source},
		{"usr-device", "readlink -f /dev/mapper/usr", &out.UsrDevice},
		{"usr-fstype", "findmnt -rn -o FSTYPE /usr", &out.FSType},
		{"usr-options", "findmnt -rn -o OPTIONS /usr", &out.Options},
		{"verity", "veritysetup status usr", &out.Verity},
		{"usr-ancestors", "lsblk -s -rno NAME /dev/mapper/usr", &out.Ancestors},
		{"partitions", "lsblk -o NAME,SIZE,FSTYPE,PARTLABEL,UUID,MOUNTPOINTS", nil},
		{"bootctl", "bootctl status --no-pager", &out.Bootctl},
		{"journal", "journalctl -b --no-pager", nil},
	}
	for _, measurement := range measurements {
		value, err := r.command(measurement.command)
		if writeErr := r.save(expected.Phase+"/"+measurement.name, []byte(value)); writeErr != nil {
			return writeErr
		}
		if err != nil {
			return fmt.Errorf("%s: %w", measurement.name, err)
		}
		if measurement.target != nil {
			*measurement.target = strings.TrimSpace(value)
		}
	}
	var err error
	if err := r.readBootTrees(&out); err != nil {
		return err
	}
	if expected.Phase != "baseline" {
		out.Status, err = r.trident("--verbosity error get status", expected.Phase+"-status", false)
		if err != nil {
			return err
		}
		if err := r.save(expected.Phase+"/status", []byte(out.Status)); err != nil {
			return err
		}
	}
	raw, err := json.MarshalIndent(out, "", "  ")
	if err != nil {
		return err
	}
	if err := r.save(expected.Phase+"/snapshot.json", raw); err != nil {
		return err
	}
	if err := validateSnapshot(out, r.snapshots[expected.Previous], expected, info); err != nil {
		return err
	}
	r.snapshots[expected.Phase] = out
	return nil
}
