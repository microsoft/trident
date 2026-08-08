package sysinspect

import "testing"

const blkidSample = `/dev/sr0: BLOCK_SIZE="2048" UUID="2023-12-16-00-55-13-99" LABEL="TRIDENT_CDROM" TYPE="iso9660"
/dev/sda4: LABEL="3e9cecef-5a01-4" UUID="37a7b4fa-87f0-4887-895b-393f46c345a0" TYPE="swap" PARTLABEL="swap" PARTUUID="3e9cecef-5a01-43d6-a1ae-58bf24f42521"
/dev/sda2: UUID="04267584-7e18-4612-a649-c71e1811bd82" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="root-a" PARTUUID="f1be3a27-36e2-4d4b-b8ec-5b0b5909cbf9"
/dev/sda1: SEC_TYPE="msdos" UUID="D920-8BA4" BLOCK_SIZE="512" TYPE="vfat" PARTLABEL="esp" PARTUUID="6fcc7c57-b21c-46e5-bc79-041c7fc53f34"
/dev/sda3: PARTLABEL="root-b" PARTUUID="573fdf4c-9133-4a9f-8cf5-aff7b74d1aeb"`

func TestParseBlkid(t *testing.T) {
	entries := ParseBlkid(blkidSample)
	if len(entries) != 5 {
		t.Fatalf("got %d entries, want 5", len(entries))
	}

	sda2, ok := entries["sda2"]
	if !ok {
		t.Fatal("missing sda2 entry")
	}
	if v, _ := sda2.Get("TYPE"); v != "ext4" {
		t.Errorf("sda2 TYPE = %q, want ext4", v)
	}
	if v, _ := sda2.Get("PARTLABEL"); v != "root-a" {
		t.Errorf("sda2 PARTLABEL = %q, want root-a", v)
	}
	if v, _ := sda2.Get("PARTUUID"); v != "f1be3a27-36e2-4d4b-b8ec-5b0b5909cbf9" {
		t.Errorf("sda2 PARTUUID = %q", v)
	}

	// sda3 has no TYPE (unformatted B volume) - Get should report absent.
	sda3 := entries["sda3"]
	if _, ok := sda3.Get("TYPE"); ok {
		t.Error("sda3 should not have TYPE")
	}
	if v, _ := sda3.Get("PARTLABEL"); v != "root-b" {
		t.Errorf("sda3 PARTLABEL = %q, want root-b", v)
	}
}

const lsblkSample = `{
  "blockdevices": [
    {"name":"sda","maj:min":"8:0","rm":false,"size":34359738368,"ro":false,"type":"disk","mountpoints":[null],
      "children":[
        {"name":"sda1","maj:min":"8:1","rm":false,"size":1073741824,"ro":false,"type":"part","mountpoints":["/boot/efi"]},
        {"name":"sda2","maj:min":"8:2","rm":false,"size":8589934592,"ro":false,"type":"part","mountpoints":["/"]}
      ]},
    {"name":"sdb","maj:min":"8:16","rm":false,"size":34359738368,"ro":false,"type":"disk","mountpoints":[null],
      "children":[
        {"name":"sdb1","maj:min":"8:17","rm":false,"size":10485760,"ro":false,"type":"part","mountpoints":[null]}
      ]},
    {"name":"sr0","maj:min":"11:0","rm":true,"size":501121024,"ro":false,"type":"rom","mountpoints":[null]}
  ]
}`

func TestParseLsblk(t *testing.T) {
	out, err := ParseLsblk(lsblkSample)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(out.Blockdevices) != 3 {
		t.Fatalf("got %d block devices, want 3", len(out.Blockdevices))
	}

	parts := out.Partitions()
	// sda1, sda2, sdb1 (children) + sr0 (no children) = 4
	if len(parts) != 4 {
		t.Fatalf("got %d partitions, want 4", len(parts))
	}

	byName := map[string]LsblkDevice{}
	for _, p := range parts {
		byName[p.Name] = p
	}
	if byName["sda2"].Size != 8589934592 {
		t.Errorf("sda2 size = %d, want 8589934592", byName["sda2"].Size)
	}
	if got := byName["sda2"].Mountpoints; len(got) != 1 || got[0] == nil || *got[0] != "/" {
		t.Errorf("sda2 mountpoint unexpected: %v", got)
	}
	if _, ok := byName["sr0"]; !ok {
		t.Error("sr0 should be treated as a leaf partition")
	}
}

const mountSample = `/dev/sda3 on / type ext4 (rw,relatime)
devtmpfs on /dev type devtmpfs (rw,nosuid)
/dev/sda5 on /home type ext4 (rw,relatime)
/dev/sda1 on /boot/efi type vfat (rw,relatime)`

func TestParseMountAndRootDevice(t *testing.T) {
	entries := ParseMount(mountSample)
	if len(entries) != 4 {
		t.Fatalf("got %d mount entries, want 4", len(entries))
	}

	root, ok := RootDevice(entries)
	if !ok {
		t.Fatal("root device not found")
	}
	if root != "/dev/sda3" {
		t.Errorf("root device = %q, want /dev/sda3", root)
	}

	if entries[0].FsType != "ext4" {
		t.Errorf("first entry fstype = %q, want ext4", entries[0].FsType)
	}
}
