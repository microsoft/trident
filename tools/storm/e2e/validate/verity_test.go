package validate

import (
	"testing"

	tridentutil "tridenttools/storm/utils/trident"
)

const verityStatusYaml = `
servicingState: provisioned
spec:
  storage:
    verity:
    - id: root
      name: root
      dataDeviceId: root-data
      hashDeviceId: root-hash
    filesystems:
    - deviceId: root
      mountPoint: /
`

func TestHasVerity(t *testing.T) {
	hs, _ := tridentutil.NewHostStatusFromYaml([]byte(verityStatusYaml))
	if !HasVerity(hs) {
		t.Error("expected HasVerity=true")
	}
	plain, _ := tridentutil.NewHostStatusFromYaml([]byte("spec:\n  storage:\n    disks: []\n"))
	if HasVerity(plain) {
		t.Error("expected HasVerity=false without verity")
	}
}

func TestVerityForRoot(t *testing.T) {
	hs, _ := tridentutil.NewHostStatusFromYaml([]byte(verityStatusYaml))
	dev, ok := verityForRoot(hs.Spec(), "root")
	if !ok {
		t.Fatal("verityForRoot not found")
	}
	if dev.Name != "root" || dev.DataDeviceID != "root-data" || dev.HashDeviceID != "root-hash" {
		t.Errorf("verity device = %+v", dev)
	}
	if _, ok := verityForRoot(hs.Spec(), "nonexistent"); ok {
		t.Error("expected not found for nonexistent root")
	}
}

func TestBasename(t *testing.T) {
	if basename("/dev/md/root-a") != "root-a" {
		t.Error("basename /dev/md/root-a")
	}
	if basename("sda1") != "sda1" {
		t.Error("basename sda1")
	}
}
