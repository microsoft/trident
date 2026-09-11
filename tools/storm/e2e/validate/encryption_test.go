package validate

import (
	"testing"

	"tridenttools/pkg/hostconfig"
	tridentutil "tridenttools/storm/utils/trident"
)

const encSpecYaml = `
storage:
  encryption:
    volumes:
    - id: enc-root
      deviceName: root
      deviceId: root-luks
    - id: enc-swap
      deviceName: swapdev
      deviceId: swap-part
  disks:
  - id: os
    partitions:
    - id: swap-part
      size: 2G
  raid:
    software:
    - id: root-luks
      name: rootarray
  abUpdate:
    volumePairs:
    - id: root
      volumeAId: enc-root
      volumeBId: enc-root-b
  filesystems:
  - deviceId: root
    mountPoint: /
  swap:
  - enc-swap
`

func encSpec(t *testing.T) hostconfig.HostConfig {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(encSpecYaml))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	return hc
}

func TestHasEncryption(t *testing.T) {
	hs, _ := tridentutil.NewHostStatusFromYaml([]byte("spec:\n" + indent(encSpecYaml)))
	if !HasEncryption(hs) {
		t.Error("expected HasEncryption=true")
	}
	plain, _ := tridentutil.NewHostStatusFromYaml([]byte("spec:\n  storage:\n    disks: []\n"))
	if HasEncryption(plain) {
		t.Error("expected HasEncryption=false")
	}
}

func TestChildAbUpdateVolumePair(t *testing.T) {
	spec := encSpec(t)
	pair, isA, ok := childAbUpdateVolumePair(spec, "enc-root")
	if !ok || !isA {
		t.Fatalf("enc-root: ok=%v isA=%v, want true,true", ok, isA)
	}
	if id, _ := pair.S("id").Data().(string); id != "root" {
		t.Errorf("pair id = %q, want root", id)
	}
	if _, isA, ok := childAbUpdateVolumePair(spec, "enc-root-b"); !ok || isA {
		t.Errorf("enc-root-b: ok=%v isA=%v, want true,false", ok, isA)
	}
	if _, _, ok := childAbUpdateVolumePair(spec, "nope"); ok {
		t.Error("nope should not be found")
	}
}

func TestFilesystemByDeviceID(t *testing.T) {
	spec := encSpec(t)
	fs, ok := filesystemByDeviceID(spec, "root")
	if !ok {
		t.Fatal("root fs not found")
	}
	if mp, ok := mountPointPath(&fs); !ok || mp != "/" {
		t.Errorf("mount = %q ok=%v, want /", mp, ok)
	}
	if _, ok := filesystemByDeviceID(spec, "missing"); ok {
		t.Error("missing fs should not be found")
	}
}

func TestIsSwapDevice(t *testing.T) {
	spec := encSpec(t)
	if !isSwapDevice(spec, "enc-swap") {
		t.Error("enc-swap should be swap")
	}
	if isSwapDevice(spec, "enc-root") {
		t.Error("enc-root should not be swap")
	}
}

func TestRaidSoftwareArrayName(t *testing.T) {
	spec := encSpec(t)
	name, ok := raidSoftwareArrayName(spec, "root-luks")
	if !ok || name != "rootarray" {
		t.Errorf("got (%q,%v), want (rootarray,true)", name, ok)
	}
	if _, ok := raidSoftwareArrayName(spec, "swap-part"); ok {
		t.Error("swap-part is a partition, not raid")
	}
}

func TestBlockDevPathByPartlabel(t *testing.T) {
	devs := map[string]map[string]string{
		"/dev/sda4": {"PARTLABEL": "swap-part", "TYPE": "crypto_LUKS"},
		"/dev/sda1": {"PARTLABEL": "esp"},
	}
	path, ok := blockDevPathByPartlabel(devs, "swap-part")
	if !ok || path != "/dev/sda4" {
		t.Errorf("got (%q,%v), want (/dev/sda4,true)", path, ok)
	}
	if _, ok := blockDevPathByPartlabel(devs, "nope"); ok {
		t.Error("nope should not match")
	}
}

// indent prefixes every non-empty line with two spaces (to nest encSpecYaml
// under a `spec:` key).
func indent(s string) string {
	out := ""
	for _, line := range splitLines(s) {
		if line == "" {
			out += "\n"
		} else {
			out += "  " + line + "\n"
		}
	}
	return out
}

func splitLines(s string) []string {
	var lines []string
	cur := ""
	for _, r := range s {
		if r == '\n' {
			lines = append(lines, cur)
			cur = ""
		} else {
			cur += string(r)
		}
	}
	if cur != "" {
		lines = append(lines, cur)
	}
	return lines
}
