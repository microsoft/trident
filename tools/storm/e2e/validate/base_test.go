package validate

import (
	"testing"

	"tridenttools/pkg/hostconfig"
	tridentutil "tridenttools/storm/utils/trident"
)

func TestParseSizeToBytes(t *testing.T) {
	cases := []struct {
		in     string
		want   int64
		wantOk bool
	}{
		{"8G", 8 * 1024 * 1024 * 1024, true},
		{"192M", 192 * 1024 * 1024, true},
		{"1K", 1024, true},
		{"512B", 512, true},
		{"1024", 1024, true},
		{"grow", 0, false},
		{"", 0, false},
		{"4.5G", int64(4.5 * 1024 * 1024 * 1024), true},
	}
	for _, c := range cases {
		got, ok := ParseSizeToBytes(c.in)
		if ok != c.wantOk || (ok && got != c.want) {
			t.Errorf("ParseSizeToBytes(%q) = (%d,%v), want (%d,%v)", c.in, got, ok, c.want, c.wantOk)
		}
	}
}

const specYaml = `
storage:
  disks:
  - id: os
    partitions:
    - id: root-a
      size: 8G
    - id: root-b
      size: 8G
    - id: esp
      size: 1G
  raid:
    software:
    - id: md-root
      name: root
  abUpdate:
    volumePairs:
    - id: root
      volumeAId: root-a
      volumeBId: root-b
  filesystems:
  - deviceId: root
    mountPoint: /
  - deviceId: esp
    mountPoint:
      path: /boot/efi
      options: umask=0077
  verity:
  - id: root
    name: root
    dataDeviceId: root-data
    hashDeviceId: root-hash
`

func mustSpec(t *testing.T) hostconfig.HostConfig {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(specYaml))
	if err != nil {
		t.Fatalf("failed to parse spec: %v", err)
	}
	return hc
}

func TestExpectedPartitions(t *testing.T) {
	parts := ExpectedPartitions(mustSpec(t))
	if len(parts) != 3 {
		t.Fatalf("got %d partitions, want 3", len(parts))
	}
	byID := map[string]PartitionExpectation{}
	for _, p := range parts {
		byID[p.ID] = p
	}
	if !byID["root-a"].HasSize || byID["root-a"].SizeBytes != 8*1024*1024*1024 {
		t.Errorf("root-a = %+v", byID["root-a"])
	}
}

func TestIsPartitionIsRaid(t *testing.T) {
	spec := mustSpec(t)
	if !IsPartition(spec, "root-a") {
		t.Error("root-a should be a partition")
	}
	if IsPartition(spec, "md-root") {
		t.Error("md-root should not be a partition")
	}
	if !IsRaid(spec, "md-root") {
		t.Error("md-root should be a raid array")
	}
	if IsRaid(spec, "root-a") {
		t.Error("root-a should not be raid")
	}
}

func TestRootFilesystemDeviceID(t *testing.T) {
	id, ok := RootFilesystemDeviceID(mustSpec(t))
	if !ok || id != "root" {
		t.Errorf("got (%q,%v), want (root,true)", id, ok)
	}
}

func TestActiveVolumeID(t *testing.T) {
	spec := mustSpec(t)
	a, ok := ActiveVolumeID(spec, "root", tridentutil.AbVolumeA)
	if !ok || a != "root-a" {
		t.Errorf("volume-a: got (%q,%v), want (root-a,true)", a, ok)
	}
	b, ok := ActiveVolumeID(spec, "root", tridentutil.AbVolumeB)
	if !ok || b != "root-b" {
		t.Errorf("volume-b: got (%q,%v), want (root-b,true)", b, ok)
	}
	if _, ok := ActiveVolumeID(spec, "nonexistent", tridentutil.AbVolumeA); ok {
		t.Error("nonexistent pair should return ok=false")
	}
}

func TestAbVolumePairID(t *testing.T) {
	spec := mustSpec(t)
	// root is a verity device in specYaml -> pair id is its dataDeviceId.
	pairID, isVerity := AbVolumePairID(spec, "root")
	if !isVerity || pairID != "root-data" {
		t.Errorf("verity root: got (%q,%v), want (root-data,true)", pairID, isVerity)
	}
	// esp is not a verity device -> pair id is the device id itself.
	pairID, isVerity = AbVolumePairID(spec, "esp")
	if isVerity || pairID != "esp" {
		t.Errorf("non-verity: got (%q,%v), want (esp,false)", pairID, isVerity)
	}
}
