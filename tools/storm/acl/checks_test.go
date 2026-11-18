package acl

import (
	"maps"
	"strings"
	"testing"
)

func snapshotFixture() (snapshot, snapshot, snapshot, checkpoint, artifactInfo) {
	baseFiles := map[string]fileDigest{"bootx64.efi": {SHA256: "base", Size: 1}}
	updateFiles := map[string]fileDigest{"bootx64.efi": {SHA256: "update", Size: 1}}
	info := artifactInfo{Variant: "standard", TargetUUID: "update-uuid", TargetHash: strings.Repeat("b", 64), TridentSHA256: strings.Repeat("c", 64)}
	base := snapshot{
		BootID: "11111111-1111-4111-8111-111111111111", Kernel: "6.18.azl3",
		OSRelease: "VARIANT_ID=azurecontainerlinux", Generation: "base", TridentSHA: info.TridentSHA256,
		UsrUUID: "base-uuid", RootHash: strings.Repeat("a", 64), Verity: "type: VERITY", Source: "/dev/mapper/usr",
		Options: "ro,relatime", Ancestors: "usr\nsda2\nsda",
		Trees: map[string]map[string]fileDigest{"BOOT": maps.Clone(baseFiles)},
	}
	reference := base
	reference.Trees = map[string]map[string]fileDigest{"BOOT": maps.Clone(baseFiles), "AZLA": maps.Clone(baseFiles), "AZLB": maps.Clone(updateFiles)}
	current := base
	current.BootID = "22222222-2222-4222-8222-222222222222"
	current.Generation = "update"
	current.UsrUUID, current.RootHash = info.TargetUUID, info.TargetHash
	current.Ancestors = "usr\nsda4\nsda"
	current.Status = "servicingState: provisioned\nabActiveVolume: volume-b\n"
	current.Bootctl = "File: /boot/EFI/AZLB/bootx64.efi"
	current.Trees = map[string]map[string]fileDigest{"BOOT": maps.Clone(updateFiles), "AZLA": maps.Clone(baseFiles), "AZLB": maps.Clone(updateFiles)}
	return current, base, reference, checkpoint{"update-1", "b", "update", "update-1-staged", "provisioned"}, info
}

func TestSnapshotRejectsWrongPhysicalSlotStateAndChangedBootFiles(t *testing.T) {
	for _, tt := range []struct {
		name   string
		change func(*snapshot)
	}{
		{"wrong physical partition", func(s *snapshot) { s.Ancestors = "usr\nsda2\nsda" }},
		{"writable usr", func(s *snapshot) { s.Options = "rw,relatime" }},
		{"wrong hash", func(s *snapshot) { s.RootHash = strings.Repeat("d", 64) }},
		{"wrong generation", func(s *snapshot) { s.Generation = "base" }},
		{"wrong binary", func(s *snapshot) { s.TridentSHA = strings.Repeat("e", 64) }},
		{"uncommitted", func(s *snapshot) { s.Status = "servicingState: ab-update-finalized\nabActiveVolume: volume-b" }},
		{"wrong status slot", func(s *snapshot) { s.Status = "servicingState: provisioned\nabActiveVolume: volume-a" }},
		{"last error", func(s *snapshot) { s.Status += "lastError: failed\n" }},
		{"changed active tree", func(s *snapshot) { s.Trees["AZLB"]["extra"] = fileDigest{Size: 1} }},
		{"changed retained tree", func(s *snapshot) { s.Trees["AZLA"]["extra"] = fileDigest{Size: 1} }},
		{"wrong fallback", func(s *snapshot) { s.Trees["BOOT"]["extra"] = fileDigest{Size: 1} }},
		{"wrong loader", func(s *snapshot) { s.Bootctl = "File: /boot/EFI/AZLA/bootx64.efi" }},
		{"unexpected mshv", func(s *snapshot) { s.MSHVPresent = true }},
	} {
		t.Run(tt.name, func(t *testing.T) {
			current, base, reference, expected, info := snapshotFixture()
			tt.change(&current)
			if err := validateSnapshot(current, base, reference, expected, info); err == nil {
				t.Fatal("accepted invalid checkpoint")
			}
		})
	}
}

func TestSnapshotAcceptsUpdateRollbackAndFinalUpdateToA(t *testing.T) {
	for _, phase := range []string{"update-1", "rollback", "update-3"} {
		t.Run(phase, func(t *testing.T) {
			current, base, reference, expected, info := snapshotFixture()
			if phase != "update-1" {
				expected.Slot = "a"
				current.Ancestors = "usr\nsda2\nsda"
				current.Status = "servicingState: provisioned\nabActiveVolume: volume-a"
				current.Bootctl = "File: /boot/EFI/AZLA/bootx64.efi"
				if phase == "rollback" {
					expected.Generation, current.Generation = "base", "base"
					current.UsrUUID, current.RootHash = base.UsrUUID, base.RootHash
				} else {
					reference.Trees["AZLA"] = maps.Clone(reference.Trees["AZLB"])
				}
				current.Trees["AZLA"] = maps.Clone(reference.Trees["AZLA"])
				current.Trees["BOOT"] = maps.Clone(current.Trees["AZLA"])
			}
			expected.Phase = phase
			if err := validateSnapshot(current, base, reference, expected, info); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestSnapshotStagingPreservesRunningBootAndSkipsBaselineStatus(t *testing.T) {
	_, base, _, _, info := snapshotFixture()
	if err := validateSnapshot(base, snapshot{}, snapshot{}, checkpoint{Phase: "baseline", Slot: "a", Generation: "base"}, info); err != nil {
		t.Fatal(err)
	}
	staged := base
	staged.Status = "servicingState: ab-update-staged\nabActiveVolume: volume-a"
	staged.Trees = map[string]map[string]fileDigest{"BOOT": maps.Clone(base.Trees["BOOT"]), "AZLA": maps.Clone(base.Trees["BOOT"]), "AZLB": {"bootx64.efi": {SHA256: "update", Size: 1}}}
	expected := checkpoint{"update-1-staged", "a", "base", "baseline", "ab-update-staged"}
	if err := validateSnapshot(staged, base, base, expected, info); err != nil {
		t.Fatal(err)
	}
	staged.BootID = "33333333-3333-4333-8333-333333333333"
	if err := validateSnapshot(staged, base, base, expected, info); err == nil {
		t.Fatal("accepted reboot during staging")
	}
}

func TestMSHVSnapshotRequiresDeviceAndCompleteSlotBundle(t *testing.T) {
	current, base, reference, expected, info := snapshotFixture()
	info.Variant = "mshv"
	current.Kernel = "6.18.mshv2"
	current.MSHVPresent, current.MSHVCharacter = true, true
	current.Bootctl = "File: /boot/EFI/AZLB/second.efi"
	for _, tree := range []map[string]fileDigest{current.Trees["AZLB"], current.Trees["BOOT"], reference.Trees["AZLB"]} {
		for _, name := range []string{"bootx64.efi", "grubx64.efi", "second.efi", "hvloader.efi", "lxhvloader.dll", "windows/system32/hvix64.exe", "windows/system32/hvax64.exe", "windows/system32/kdstub.dll"} {
			tree[name] = fileDigest{SHA256: "mshv", Size: 1}
		}
	}
	if err := validateSnapshot(current, base, reference, expected, info); err != nil {
		t.Fatal(err)
	}
	current.MSHVCharacter = false
	if err := validateSnapshot(current, base, reference, expected, info); err == nil {
		t.Fatal("accepted regular /dev/mshv file")
	}
	current.MSHVCharacter = true
	for _, tree := range []map[string]fileDigest{current.Trees["AZLB"], current.Trees["BOOT"], reference.Trees["AZLB"]} {
		delete(tree, "lxhvloader.dll")
	}
	if err := validateSnapshot(current, base, reference, expected, info); err == nil {
		t.Fatal("accepted incomplete MSHV bundle")
	}
}
