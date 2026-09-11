package trident

import (
	"strings"
	"testing"
)

const sampleHostStatusYaml = `
abActiveVolume: volume-a
diskUuids:
  disk-0: f4265b47-09cd-4d5e-aa92-684fb783f817
installIndex: 0
partitionPaths:
  esp: /dev/sda1
  root-a: /dev/sda2
  root-b: /dev/sda3
servicingState: provisioned
lastError: null
spec:
  storage:
    abUpdate:
      volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
    disks:
    - device: /dev/sda
      id: disk-0
      partitions:
      - id: esp
        size: 8M
        type: esp
      - id: root-a
        size: 4G
        type: linux-generic
  image:
    url: http://blob/regular.cosi
    contents: !image
      sha256: abc123
      length: 705090048
`

func TestNewHostStatusFromYaml_CoreFields(t *testing.T) {
	hs, err := NewHostStatusFromYaml([]byte(sampleHostStatusYaml))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if got := hs.ServicingState(); got != ServicingStateProvisioned {
		t.Errorf("ServicingState() = %q, want %q", got, ServicingStateProvisioned)
	}

	vol, present := hs.AbActiveVolume()
	if !present {
		t.Fatal("AbActiveVolume() reported absent, want present")
	}
	if vol != AbVolumeA {
		t.Errorf("AbActiveVolume() = %q, want %q", vol, AbVolumeA)
	}

	paths := hs.PartitionPaths()
	if len(paths) != 3 {
		t.Errorf("PartitionPaths() len = %d, want 3", len(paths))
	}
	if paths["esp"] != "/dev/sda1" {
		t.Errorf("PartitionPaths()[esp] = %q, want /dev/sda1", paths["esp"])
	}
	if paths["root-a"] != "/dev/sda2" {
		t.Errorf("PartitionPaths()[root-a] = %q, want /dev/sda2", paths["root-a"])
	}
}

func TestNewHostStatusFromYaml_CustomImageTag(t *testing.T) {
	// The `!image` tag on `spec.image.contents` must decode into a plain map,
	// reachable via the gabs escape hatch through Spec().
	hs, err := NewHostStatusFromYaml([]byte(sampleHostStatusYaml))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	spec := hs.Spec()
	if got := spec.S("image", "contents", "sha256").Data(); got != "abc123" {
		t.Errorf("spec image contents sha256 = %v, want abc123", got)
	}
	if !spec.HasABUpdate() {
		t.Error("Spec().HasABUpdate() = false, want true")
	}
}

func TestHostStatus_AbActiveVolumeAbsent(t *testing.T) {
	hs, err := NewHostStatusFromYaml([]byte("servicingState: not-provisioned\n"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if got := hs.ServicingState(); got != ServicingStateNotProvisioned {
		t.Errorf("ServicingState() = %q, want %q", got, ServicingStateNotProvisioned)
	}
	if _, present := hs.AbActiveVolume(); present {
		t.Error("AbActiveVolume() reported present, want absent")
	}
}

func TestHostStatus_LastError(t *testing.T) {
	hs, err := NewHostStatusFromYaml([]byte(sampleHostStatusYaml))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if _, present := hs.LastError(); present {
		t.Error("null lastError should be reported as absent")
	}

	withErr, err := NewHostStatusFromYaml([]byte("lastError:\n  message: Failed health check(s)\n"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	msg, present := withErr.LastError()
	if !present {
		t.Fatal("LastError() reported absent, want present")
	}
	if !strings.Contains(msg, "Failed health check(s)") {
		t.Errorf("LastError() = %q, want it to contain 'Failed health check(s)'", msg)
	}
}

func TestAbVolumeSelection_Other(t *testing.T) {
	if got := AbVolumeA.Other(); got != AbVolumeB {
		t.Errorf("AbVolumeA.Other() = %q, want %q", got, AbVolumeB)
	}
	if got := AbVolumeB.Other(); got != AbVolumeA {
		t.Errorf("AbVolumeB.Other() = %q, want %q", got, AbVolumeA)
	}
}
