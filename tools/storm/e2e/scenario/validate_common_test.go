package scenario

import (
	"testing"
)

// --- ParseBlkid tests ---

func TestParseBlkid_BasicOutput(t *testing.T) {
	input := `/dev/sda1: UUID="D920-8BA4" BLOCK_SIZE="512" TYPE="vfat" PARTLABEL="esp" PARTUUID="6fcc7c57-b21c-46e5-bc79-041c7fc53f34"
/dev/sda2: UUID="04267584-7e18-4612-a649-c71e1811bd82" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="root-a" PARTUUID="f1be3a27-36e2-4d4b-b8ec-5b0b5909cbf9"
/dev/sda3: PARTLABEL="root-b" PARTUUID="573fdf4c-9133-4a9f-8cf5-aff7b74d1aeb"
/dev/sr0: BLOCK_SIZE="2048" UUID="2023-12-16-00-55-13-99" LABEL="TRIDENT_CDROM" TYPE="iso9660"`

	entries := ParseBlkid(input)

	if len(entries) != 4 {
		t.Fatalf("expected 4 entries, got %d", len(entries))
	}

	sda1 := entries["sda1"]
	if sda1.Properties["TYPE"] != "vfat" {
		t.Errorf("sda1 TYPE: expected 'vfat', got %q", sda1.Properties["TYPE"])
	}
	if sda1.Properties["PARTLABEL"] != "esp" {
		t.Errorf("sda1 PARTLABEL: expected 'esp', got %q", sda1.Properties["PARTLABEL"])
	}
	if sda1.DevicePath != "/dev/sda1" {
		t.Errorf("sda1 DevicePath: expected '/dev/sda1', got %q", sda1.DevicePath)
	}

	sda3 := entries["sda3"]
	if sda3.Properties["PARTLABEL"] != "root-b" {
		t.Errorf("sda3 PARTLABEL: expected 'root-b', got %q", sda3.Properties["PARTLABEL"])
	}
	if _, ok := sda3.Properties["UUID"]; ok {
		t.Error("sda3 should not have UUID")
	}
}

func TestParseBlkid_DevMapper(t *testing.T) {
	input := `/dev/mapper/root: UUID="aeca4bee-73f3-4ae0-aaa3-57ae0a29ee4b" BLOCK_SIZE="4096" TYPE="ext4"`

	entries := ParseBlkid(input)

	root, ok := entries["root"]
	if !ok {
		t.Fatal("expected 'root' entry for /dev/mapper/root")
	}
	if root.Properties["TYPE"] != "ext4" {
		t.Errorf("root TYPE: expected 'ext4', got %q", root.Properties["TYPE"])
	}
}

func TestParseBlkid_EmptyInput(t *testing.T) {
	entries := ParseBlkid("")
	if len(entries) != 0 {
		t.Errorf("expected 0 entries for empty input, got %d", len(entries))
	}
}

// --- ParseBlkidExport tests ---

func TestParseBlkidExport_BasicOutput(t *testing.T) {
	input := `DEVNAME=/dev/md127
UUID=475f0351-4bb7-49bb-b9af-1f53f94b91cb
TYPE=crypto_LUKS

DEVNAME=/dev/sr0
BLOCK_SIZE=2048
UUID=2024-10-30-22-05-47-00
LABEL=CDROM
TYPE=iso9660`

	devs := ParseBlkidExport(input)

	if len(devs) != 2 {
		t.Fatalf("expected 2 devices, got %d", len(devs))
	}

	md127 := devs["/dev/md127"]
	if md127["TYPE"] != "crypto_LUKS" {
		t.Errorf("md127 TYPE: expected 'crypto_LUKS', got %q", md127["TYPE"])
	}
	if md127["UUID"] != "475f0351-4bb7-49bb-b9af-1f53f94b91cb" {
		t.Errorf("md127 UUID: expected '475f0351-...', got %q", md127["UUID"])
	}

	sr0 := devs["/dev/sr0"]
	if sr0["LABEL"] != "CDROM" {
		t.Errorf("sr0 LABEL: expected 'CDROM', got %q", sr0["LABEL"])
	}
}

func TestParseBlkidExport_EmptyInput(t *testing.T) {
	devs := ParseBlkidExport("")
	if len(devs) != 0 {
		t.Errorf("expected 0 devices for empty input, got %d", len(devs))
	}
}

// --- ParseLsblk tests ---

func TestParseLsblk_BasicOutput(t *testing.T) {
	input := `{
    "blockdevices": [
        {
            "name": "sda",
            "maj:min": "8:0",
            "rm": false,
            "size": "34359738368",
            "ro": false,
            "type": "disk",
            "mountpoints": [null],
            "children": [
                {
                    "name": "sda1",
                    "maj:min": "8:1",
                    "rm": false,
                    "size": "1073741824",
                    "ro": false,
                    "type": "part",
                    "mountpoints": ["/boot/efi"]
                },
                {
                    "name": "sda2",
                    "maj:min": "8:2",
                    "rm": false,
                    "size": "8589934592",
                    "ro": false,
                    "type": "part",
                    "mountpoints": ["/"]
                }
            ]
        },
        {
            "name": "sr0",
            "maj:min": "11:0",
            "rm": true,
            "size": "500000000",
            "ro": false,
            "type": "rom",
            "mountpoints": [null]
        }
    ]
}`

	output, err := ParseLsblk(input)
	if err != nil {
		t.Fatalf("ParseLsblk failed: %v", err)
	}

	if len(output.BlockDevices) != 2 {
		t.Fatalf("expected 2 block devices, got %d", len(output.BlockDevices))
	}

	sda := output.BlockDevices[0]
	if sda.Name != "sda" {
		t.Errorf("expected first device 'sda', got %q", sda.Name)
	}
	if len(sda.Children) != 2 {
		t.Errorf("expected 2 children for sda, got %d", len(sda.Children))
	}

	partitions := output.FlattenPartitions()
	if len(partitions) != 3 {
		t.Fatalf("expected 3 partitions, got %d", len(partitions))
	}
	if partitions[0].Name != "sda1" {
		t.Errorf("expected first partition 'sda1', got %q", partitions[0].Name)
	}
}

func TestParseLsblk_InvalidJSON(t *testing.T) {
	_, err := ParseLsblk("not json")
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}

// --- ParseMount tests ---

func TestParseMount_BasicOutput(t *testing.T) {
	input := `/dev/sda3 on / type ext4 (rw,relatime)
devtmpfs on /dev type devtmpfs (rw,nosuid,size=4096k,nr_inodes=721913,mode=755)
/dev/sda5 on /home type ext4 (rw,relatime)
/dev/sda1 on /boot/efi type vfat (rw,relatime,fmask=0077,dmask=0077)`

	entries := ParseMount(input)
	if len(entries) != 4 {
		t.Fatalf("expected 4 entries, got %d", len(entries))
	}

	root := FindRootDevice(entries)
	if root != "/dev/sda3" {
		t.Errorf("expected root device '/dev/sda3', got %q", root)
	}

	if entries[2].MountPoint != "/home" {
		t.Errorf("expected mount point '/home', got %q", entries[2].MountPoint)
	}
}

func TestParseMount_NoRoot(t *testing.T) {
	input := `/dev/sda1 on /boot type ext4 (rw)`
	entries := ParseMount(input)
	root := FindRootDevice(entries)
	if root != "" {
		t.Errorf("expected empty root device, got %q", root)
	}
	_ = entries
}

// --- ParsePasswd tests ---

func TestParsePasswd_BasicOutput(t *testing.T) {
	input := `root:x:0:0:root:/root:/bin/bash
bin:x:1:1:bin:/dev/null:/bin/false
testing-user:x:1001:1001::/home/testing-user:/bin/bash`

	entries := ParsePasswd(input)
	if len(entries) != 3 {
		t.Fatalf("expected 3 entries, got %d", len(entries))
	}

	user := entries["testing-user"]
	if user.UID != "1001" {
		t.Errorf("expected UID '1001', got %q", user.UID)
	}
	if user.Home != "/home/testing-user" {
		t.Errorf("expected home '/home/testing-user', got %q", user.Home)
	}
}

func TestParsePasswd_SkipsComments(t *testing.T) {
	input := `# comment line
root:x:0:0:root:/root:/bin/bash`

	entries := ParsePasswd(input)
	if len(entries) != 1 {
		t.Errorf("expected 1 entry, got %d", len(entries))
	}
}

// --- ParseGroup tests ---

func TestParseGroup_BasicOutput(t *testing.T) {
	input := `root:x:0:
bin:x:1:daemon
wheel:x:10:testing-user,admin`

	entries := ParseGroup(input)
	if len(entries) != 3 {
		t.Fatalf("expected 3 entries, got %d", len(entries))
	}

	wheel := entries["wheel"]
	if len(wheel.Members) != 2 {
		t.Fatalf("expected 2 members in wheel, got %d", len(wheel.Members))
	}
	if wheel.Members[0] != "testing-user" {
		t.Errorf("expected first member 'testing-user', got %q", wheel.Members[0])
	}

	root := entries["root"]
	if len(root.Members) != 0 {
		t.Errorf("expected 0 members in root, got %d", len(root.Members))
	}
}

// --- ParseEfiBootMgr tests ---

func TestParseEfiBootMgr_BasicOutput(t *testing.T) {
	input := `BootCurrent: 0001
Timeout: 0 seconds
BootOrder: 0001,0000
Boot0000* EFI DVD/CDROM
Boot0001* Azure Linux`

	info := ParseEfiBootMgr(input)

	if info.BootCurrent != "0001" {
		t.Errorf("expected BootCurrent '0001', got %q", info.BootCurrent)
	}
	if len(info.BootEntries) != 2 {
		t.Fatalf("expected 2 boot entries, got %d", len(info.BootEntries))
	}
	if info.BootEntries["0001"] != "Azure Linux" {
		t.Errorf("expected Boot0001 'Azure Linux', got %q", info.BootEntries["0001"])
	}

	name := info.CurrentBootName()
	if name != "Azure" {
		t.Errorf("expected current boot name 'Azure', got %q", name)
	}
}

// --- ParseKeyValueLines tests ---

func TestParseKeyValueLines_CryptsetupStatus(t *testing.T) {
	input := `  type:    n/a
  cipher:  aes-xts-plain64
  keysize: 512 bits
  key location: keyring
  device:  /dev/md127
  sector size:  512
  offset:  16384 sectors
  size:    2080640 sectors
  mode:    read/write`

	result := ParseKeyValueLines(input)

	if result["cipher"] != "aes-xts-plain64" {
		t.Errorf("cipher: expected 'aes-xts-plain64', got %q", result["cipher"])
	}
	if result["keysize"] != "512 bits" {
		t.Errorf("keysize: expected '512 bits', got %q", result["keysize"])
	}
	if result["mode"] != "read/write" {
		t.Errorf("mode: expected 'read/write', got %q", result["mode"])
	}
}

func TestParseKeyValueLines_DmsetupInfo(t *testing.T) {
	input := `Name:              web
State:             ACTIVE
Read Ahead:        256
Tables present:    LIVE
Open count:        0
Event number:      0
Major, minor:      254, 0
Number of targets: 1
UUID: CRYPT-LUKS2-475f03514bb749bbb9af1f53f94b91cb-web`

	result := ParseKeyValueLines(input)

	if result["Name"] != "web" {
		t.Errorf("Name: expected 'web', got %q", result["Name"])
	}
	if result["State"] != "ACTIVE" {
		t.Errorf("State: expected 'ACTIVE', got %q", result["State"])
	}
	if result["UUID"] != "CRYPT-LUKS2-475f03514bb749bbb9af1f53f94b91cb-web" {
		t.Errorf("UUID: expected 'CRYPT-LUKS2-...-web', got %q", result["UUID"])
	}
}

// --- ParseTable tests ---

func TestParseTable_FindmntOutput(t *testing.T) {
	input := `TARGET   SOURCE           FSTYPE OPTIONS
/mnt/web /dev/mapper/web  ext4   rw,relatime`

	rows := ParseTable(input)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	if rows[0]["TARGET"] != "/mnt/web" {
		t.Errorf("TARGET: expected '/mnt/web', got %q", rows[0]["TARGET"])
	}
	if rows[0]["SOURCE"] != "/dev/mapper/web" {
		t.Errorf("SOURCE: expected '/dev/mapper/web', got %q", rows[0]["SOURCE"])
	}
}

func TestParseTable_EmptyInput(t *testing.T) {
	rows := ParseTable("")
	if rows != nil {
		t.Errorf("expected nil for empty input, got %v", rows)
	}
}

// --- ParseDevMdListing tests ---

func TestParseDevMdListing_BasicOutput(t *testing.T) {
	input := `lrwxrwxrwx 1 root root 8 Apr  1 22:42 home -> ../md124
lrwxrwxrwx 1 root root 8 Apr  1 22:42 root-a -> ../md127
lrwxrwxrwx 1 root root 8 Apr  1 22:42 root-b -> ../md125
lrwxrwxrwx 1 root root 8 Apr  1 22:42 trident -> ../md126`

	result := ParseDevMdListing(input)

	if len(result) != 4 {
		t.Fatalf("expected 4 entries, got %d", len(result))
	}
	if result["md127"] != "/dev/md/root-a" {
		t.Errorf("md127: expected '/dev/md/root-a', got %q", result["md127"])
	}
	if result["md124"] != "/dev/md/home" {
		t.Errorf("md124: expected '/dev/md/home', got %q", result["md124"])
	}
}

func TestParseDevMdListing_EmptyInput(t *testing.T) {
	result := ParseDevMdListing("")
	if len(result) != 0 {
		t.Errorf("expected 0 entries for empty input, got %d", len(result))
	}
}

// --- ParseVeritySetupStatus tests ---

func TestParseVeritySetupStatus_BasicOutput(t *testing.T) {
	input := `/dev/mapper/root is active and is in use.
  type:        VERITY
  status:      verified
  hash type:   1
  data block:  4096
  hash block:  4096
  hash name:   sha256
  salt:        95c671631e5202431ead38146e1af8342100ff03bc2a89f2590dcb3454cc6e31
  data device: /dev/sda3
  size:        1377128 sectors
  mode:        readonly
  hash device: /dev/sda4
  hash offset: 8 sectors
  root hash:   a8c34ed685f365352231db21aa36ff23bf8b658e001afa8e498f57d1755e9a19
  flags:       panic_on_corruption`

	status := ParseVeritySetupStatus(input)

	if !status.IsActive {
		t.Error("expected IsActive to be true")
	}
	if !status.IsInUse {
		t.Error("expected IsInUse to be true")
	}
	if status.Properties["type"] != "VERITY" {
		t.Errorf("type: expected 'VERITY', got %q", status.Properties["type"])
	}
	if status.Properties["status"] != "verified" {
		t.Errorf("status: expected 'verified', got %q", status.Properties["status"])
	}
	if status.Properties["mode"] != "readonly" {
		t.Errorf("mode: expected 'readonly', got %q", status.Properties["mode"])
	}
	if status.DataDevice != "/dev/sda3" {
		t.Errorf("DataDevice: expected '/dev/sda3', got %q", status.DataDevice)
	}
	if status.HashDevice != "/dev/sda4" {
		t.Errorf("HashDevice: expected '/dev/sda4', got %q", status.HashDevice)
	}
}

func TestParseVeritySetupStatus_RaidDevices(t *testing.T) {
	input := `/dev/mapper/root is active and is in use.
  type:        VERITY
  status:      verified
  data device: /dev/md126
  mode:        readonly
  hash device: /dev/md127`

	status := ParseVeritySetupStatus(input)
	if status.DataDevice != "/dev/md126" {
		t.Errorf("DataDevice: expected '/dev/md126', got %q", status.DataDevice)
	}
	if status.HashDevice != "/dev/md127" {
		t.Errorf("HashDevice: expected '/dev/md127', got %q", status.HashDevice)
	}
}

// --- ParseLuksDump tests ---

func TestParseLuksDump_BasicOutput(t *testing.T) {
	input := `{
    "keyslots": {
        "1": {
            "type": "luks2",
            "key_size": 64,
            "kdf": {
                "type": "pbkdf2",
                "hash": "sha512",
                "iterations": 1000,
                "salt": "FHJf95bq+nk/WkCCCOIyPDwLbzpwkkiTgs2vjFZgLU0="
            },
            "area": {
                "type": "raw",
                "encryption": "aes-xts-plain64",
                "key_size": 64
            }
        }
    },
    "tokens": {
        "0": {
            "type": "systemd-tpm2",
            "keyslots": ["1"],
            "tpm2-pcrs": [],
            "tpm2_pcrlock": true
        }
    },
    "segments": {
        "0": {
            "type": "crypt",
            "encryption": "aes-xts-plain64",
            "sector_size": 512
        }
    },
    "digests": {
        "0": {
            "type": "pbkdf2",
            "hash": "sha512"
        }
    },
    "config": {
        "json_size": "12288",
        "keyslots_size": "16744448"
    }
}`

	dump, err := ParseLuksDump(input)
	if err != nil {
		t.Fatalf("ParseLuksDump failed: %v", err)
	}

	if len(dump.Keyslots) != 1 {
		t.Fatalf("expected 1 keyslot, got %d", len(dump.Keyslots))
	}
	ks := dump.Keyslots["1"]
	if ks.Type != "luks2" {
		t.Errorf("keyslot type: expected 'luks2', got %q", ks.Type)
	}
	if ks.KDF.Type != "pbkdf2" {
		t.Errorf("KDF type: expected 'pbkdf2', got %q", ks.KDF.Type)
	}
	if ks.KDF.Hash != "sha512" {
		t.Errorf("KDF hash: expected 'sha512', got %q", ks.KDF.Hash)
	}

	if len(dump.Tokens) != 1 {
		t.Fatalf("expected 1 token, got %d", len(dump.Tokens))
	}
	tok := dump.Tokens["0"]
	if tok.Type != "systemd-tpm2" {
		t.Errorf("token type: expected 'systemd-tpm2', got %q", tok.Type)
	}
	if tok.TPM2PCRLock == nil || !*tok.TPM2PCRLock {
		t.Error("expected tpm2_pcrlock to be true")
	}
	if len(tok.TPM2PCRs) != 0 {
		t.Errorf("expected empty tpm2-pcrs, got %v", tok.TPM2PCRs)
	}

	if dump.Digests["0"].Type != "pbkdf2" {
		t.Errorf("digest type: expected 'pbkdf2', got %q", dump.Digests["0"].Type)
	}
	if dump.Digests["0"].Hash != "sha512" {
		t.Errorf("digest hash: expected 'sha512', got %q", dump.Digests["0"].Hash)
	}
}

func TestParseLuksDump_NonUkiPCRs(t *testing.T) {
	input := `{
    "keyslots": {},
    "tokens": {
        "0": {
            "type": "systemd-tpm2",
            "keyslots": ["1"],
            "tpm2-pcrs": [7],
            "tpm2_pcrlock": false
        }
    },
    "segments": {},
    "digests": {},
    "config": {
        "json_size": "12288",
        "keyslots_size": "16744448"
    }
}`

	dump, err := ParseLuksDump(input)
	if err != nil {
		t.Fatalf("ParseLuksDump failed: %v", err)
	}

	tok := dump.Tokens["0"]
	if tok.TPM2PCRLock == nil || *tok.TPM2PCRLock {
		t.Error("expected tpm2_pcrlock to be false")
	}
	if len(tok.TPM2PCRs) != 1 || tok.TPM2PCRs[0] != 7 {
		t.Errorf("expected tpm2-pcrs [7], got %v", tok.TPM2PCRs)
	}
}

// --- ParseSysextStatus tests ---

func TestParseSysextStatus_BasicOutput(t *testing.T) {
	input := `[
    {
        "hierarchy": "/usr",
        "extensions": ["my-sysext", "another-ext"]
    },
    {
        "hierarchy": "/opt",
        "extensions": ["opt-ext"]
    }
]`

	hierarchies, err := ParseSysextStatus(input)
	if err != nil {
		t.Fatalf("ParseSysextStatus failed: %v", err)
	}

	if len(hierarchies) != 2 {
		t.Fatalf("expected 2 hierarchies, got %d", len(hierarchies))
	}

	allExts := AllActiveExtensions(hierarchies)
	if len(allExts) != 3 {
		t.Fatalf("expected 3 extensions, got %d", len(allExts))
	}

	expected := map[string]bool{"my-sysext": true, "another-ext": true, "opt-ext": true}
	for _, ext := range allExts {
		if !expected[ext] {
			t.Errorf("unexpected extension %q", ext)
		}
	}
}

func TestParseSysextStatus_EmptyHierarchies(t *testing.T) {
	input := `[{"hierarchy": "/usr", "extensions": []}]`

	hierarchies, err := ParseSysextStatus(input)
	if err != nil {
		t.Fatalf("ParseSysextStatus failed: %v", err)
	}

	allExts := AllActiveExtensions(hierarchies)
	if len(allExts) != 0 {
		t.Errorf("expected 0 extensions, got %d", len(allExts))
	}
}

// --- ParseTridentGetOutput tests ---

func TestParseTridentGetOutput_Basic(t *testing.T) {
	input := `servicingState: provisioned
abActiveVolume: volume-a
partitionPaths:
  root-a: /dev/disk/by-partuuid/f1be3a27
  root-b: /dev/disk/by-partuuid/573fdf4c`

	result, err := ParseTridentGetOutput(input)
	if err != nil {
		t.Fatalf("ParseTridentGetOutput failed: %v", err)
	}

	if result["servicingState"] != "provisioned" {
		t.Errorf("servicingState: expected 'provisioned', got %v", result["servicingState"])
	}
	if result["abActiveVolume"] != "volume-a" {
		t.Errorf("abActiveVolume: expected 'volume-a', got %v", result["abActiveVolume"])
	}
}
