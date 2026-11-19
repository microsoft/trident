package validate

import (
	"strings"
	"testing"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/sysinspect"
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

const raidABSpecYaml = `
storage:
  disks:
  - id: os
    partitions:
    - id: root-a1
    - id: root-b1
  - id: disk2
    partitions:
    - id: root-a2
    - id: root-b2
  raid:
    software:
    - id: root-a
      name: root-a
      devices:
      - root-a1
      - root-a2
    - id: root-b
      name: root-b
      devices:
      - root-b1
      - root-b2
  abUpdate:
    volumePairs:
    - id: root
      volumeAId: root-a
      volumeBId: root-b
  filesystems:
  - deviceId: root
    mountPoint: /
`

func TestValidateActiveVolumePathRaidAssertsMountedArray(t *testing.T) {
	hs := activeVolumeHostStatus(t, raidABSpecYaml)
	if _, ok := hs.PartitionPaths()["root-a"]; ok {
		t.Fatal("test Host Status must omit the RAID array id to cover the old no-op path")
	}

	var sa SoftAsserter
	validateActiveVolumePathFromMount(
		&sa,
		hs,
		hs.Spec(),
		nil,
		tridentutil.AbVolumeA,
		"/dev/md127",
		func(device string) (string, bool, error) {
			if device != "/dev/md127" {
				t.Errorf("resolved RAID for %q, want /dev/md127", device)
			}
			return "/dev/md/root-a", true, nil
		})

	summary := sa.Summary()
	if sa.HasFailures() {
		t.Fatalf("unexpected failures:\n%s", summary)
	}
	if !strings.Contains(summary, "PASS  partitions/ab-raid-path-match") {
		t.Fatalf("RAID path match was not recorded:\n%s", summary)
	}
}

func TestValidateActiveVolumePathRaidFailsWrongMountedArray(t *testing.T) {
	hs := activeVolumeHostStatus(t, raidABSpecYaml)

	var sa SoftAsserter
	validateActiveVolumePathFromMount(
		&sa,
		hs,
		hs.Spec(),
		nil,
		tridentutil.AbVolumeA,
		"/dev/md127",
		func(string) (string, bool, error) {
			return "/dev/md/root-b", true, nil
		})

	summary := sa.Summary()
	if !sa.HasFailures() {
		t.Fatalf("expected wrong mounted RAID array to fail:\n%s", summary)
	}
	if !strings.Contains(summary, "FAIL  partitions/ab-raid-path-match") {
		t.Fatalf("wrong RAID array failed under the wrong sub-check:\n%s", summary)
	}
}

func TestValidateActiveVolumePathPartitionMissingStatusPathFails(t *testing.T) {
	const partitionABSpecYaml = `
storage:
  disks:
  - id: os
    partitions:
    - id: root-a
    - id: root-b
  abUpdate:
    volumePairs:
    - id: root
      volumeAId: root-a
      volumeBId: root-b
  filesystems:
  - deviceId: root
    mountPoint: /
`
	hs := activeVolumeHostStatus(t, partitionABSpecYaml)
	blkid := map[string]sysinspect.BlkidEntry{
		"sda2": {Fields: map[string]string{"PARTUUID": "active-partuuid"}},
	}

	var sa SoftAsserter
	validateActiveVolumePathFromMount(
		&sa,
		hs,
		hs.Spec(),
		blkid,
		tridentutil.AbVolumeA,
		"/dev/sda2",
		nil)

	summary := sa.Summary()
	if !sa.HasFailures() {
		t.Fatalf("expected missing active partition path to fail:\n%s", summary)
	}
	if !strings.Contains(summary, "active partition volume \"root-a\" missing") {
		t.Fatalf("missing partition path failure was not recorded:\n%s", summary)
	}
}

func activeVolumeHostStatus(t *testing.T, spec string) tridentutil.HostStatus {
	t.Helper()
	hs, err := tridentutil.NewHostStatusFromYaml([]byte(`
abActiveVolume: volume-a
partitionPaths:
  root-a1: /dev/disk/by-partuuid/root-a1
  root-a2: /dev/disk/by-partuuid/root-a2
  root-b1: /dev/disk/by-partuuid/root-b1
  root-b2: /dev/disk/by-partuuid/root-b2
spec:
` + indentForHostStatus(spec)))
	if err != nil {
		t.Fatalf("parse Host Status: %v", err)
	}
	return hs
}

func indentForHostStatus(s string) string {
	var out strings.Builder
	for _, line := range strings.Split(s, "\n") {
		if line == "" {
			out.WriteByte('\n')
			continue
		}
		out.WriteString("  ")
		out.WriteString(line)
		out.WriteByte('\n')
	}
	return out.String()
}
