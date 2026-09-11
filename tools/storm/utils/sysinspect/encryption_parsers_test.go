package sysinspect

import "testing"

func TestParseCryptsetupStatus_InUse(t *testing.T) {
	sample := `/dev/mapper/web is active and is in use.
  type:    n/a
  cipher:  aes-xts-plain64
  keysize: 512 bits
  device:  /dev/md127
  mode:    read/write`
	s := ParseCryptsetupStatus(sample, "web")
	if !s.Active || !s.InUse {
		t.Errorf("Active=%v InUse=%v, want both true", s.Active, s.InUse)
	}
	if v, _ := s.Get("cipher"); v != "aes-xts-plain64" {
		t.Errorf("cipher = %q", v)
	}
	if v, _ := s.Get("keysize"); v != "512 bits" {
		t.Errorf("keysize = %q", v)
	}
}

func TestParseCryptsetupStatus_ActiveNotInUse(t *testing.T) {
	s := ParseCryptsetupStatus("/dev/mapper/web is active.\n  cipher:  aes-xts-plain64", "web")
	if !s.Active || s.InUse {
		t.Errorf("Active=%v InUse=%v, want active-not-inuse", s.Active, s.InUse)
	}
}

func TestParseLuksDump(t *testing.T) {
	sample := `{
	  "keyslots": {"1": {"type":"luks2","kdf":{"type":"pbkdf2","hash":"sha512"},"area":{"encryption":"aes-xts-plain64"}}},
	  "tokens": {"0": {"type":"systemd-tpm2","keyslots":["1"],"tpm2_pcrlock":false,"tpm2-pcrs":[7]}},
	  "digests": {"0": {"type":"pbkdf2","hash":"sha512"}}
	}`
	d, err := ParseLuksDump(sample)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if d.Keyslots["1"].Type != "luks2" || d.Keyslots["1"].Kdf.Hash != "sha512" {
		t.Errorf("keyslot 1 = %+v", d.Keyslots["1"])
	}
	if d.Keyslots["1"].Area.Encryption != "aes-xts-plain64" {
		t.Errorf("area encryption = %q", d.Keyslots["1"].Area.Encryption)
	}
	tok := d.Tokens["0"]
	if tok.Type != "systemd-tpm2" || tok.Tpm2Pcrlock != false || len(tok.Tpm2Pcrs) != 1 || tok.Tpm2Pcrs[0] != 7 {
		t.Errorf("token 0 = %+v", tok)
	}
	if d.Digests["0"].Type != "pbkdf2" || d.Digests["0"].Hash != "sha512" {
		t.Errorf("digest 0 = %+v", d.Digests["0"])
	}
}

func TestParseKeyValueLines(t *testing.T) {
	sample := `Name:              web
State:             ACTIVE
Tables present:    LIVE
UUID: CRYPT-LUKS2-475f03514bb749bbb9af1f53f94b91cb-web`
	m := ParseKeyValueLines(sample)
	if m["Name"] != "web" || m["State"] != "ACTIVE" || m["Tables present"] != "LIVE" {
		t.Errorf("parsed = %v", m)
	}
	if m["UUID"] != "CRYPT-LUKS2-475f03514bb749bbb9af1f53f94b91cb-web" {
		t.Errorf("UUID = %q", m["UUID"])
	}
}

func TestParseFindmnt(t *testing.T) {
	sample := `TARGET SOURCE FSTYPE OPTIONS
/mnt/web /dev/mapper/web ext4 rw,relatime`
	rows := ParseFindmnt(sample)
	if len(rows) != 1 {
		t.Fatalf("got %d rows, want 1", len(rows))
	}
	r := rows[0]
	if r.Target != "/mnt/web" || r.Source != "/dev/mapper/web" || r.FsType != "ext4" {
		t.Errorf("row = %+v", r)
	}
}

func TestParseBlkidExport(t *testing.T) {
	sample := `DEVNAME=/dev/md127
UUID=475f0351-4bb7-49bb-b9af-1f53f94b91cb
TYPE=crypto_LUKS
PARTLABEL=web

DEVNAME=/dev/sr0
TYPE=iso9660`
	devs := ParseBlkidExport(sample)
	if len(devs) != 2 {
		t.Fatalf("got %d devices, want 2", len(devs))
	}
	if devs["/dev/md127"]["TYPE"] != "crypto_LUKS" {
		t.Errorf("md127 TYPE = %q", devs["/dev/md127"]["TYPE"])
	}
	if devs["/dev/md127"]["PARTLABEL"] != "web" {
		t.Errorf("md127 PARTLABEL = %q", devs["/dev/md127"]["PARTLABEL"])
	}
}
