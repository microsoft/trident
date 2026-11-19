package sysinspect

import "testing"

const veritysetupSample = `/dev/mapper/root is active and is in use.
  type:        VERITY
  status:      verified
  hash type:   1
  data block:  4096
  hash block:  4096
  hash name:   sha256
  data device: /dev/sda3
  size:        1377128 sectors
  mode:        readonly
  hash device: /dev/sda4
  hash offset: 8 sectors
  root hash:   a8c34ed685f365352231db21aa36ff23bf8b658e001afa8e498f57d1755e9a19
  flags:       panic_on_corruption`

func TestParseVeritySetupStatus(t *testing.T) {
	s := ParseVeritySetupStatus(veritysetupSample, "root")
	if !s.Active {
		t.Error("expected Active=true")
	}
	if v, _ := s.Get("type"); v != "VERITY" {
		t.Errorf("type = %q, want VERITY", v)
	}
	if v, _ := s.Get("status"); v != "verified" {
		t.Errorf("status = %q, want verified", v)
	}
	if v, _ := s.Get("mode"); v != "readonly" {
		t.Errorf("mode = %q, want readonly", v)
	}
	if v, _ := s.Get("data device"); v != "/dev/sda3" {
		t.Errorf("data device = %q, want /dev/sda3", v)
	}
	if v, _ := s.Get("hash device"); v != "/dev/sda4" {
		t.Errorf("hash device = %q, want /dev/sda4", v)
	}
}

func TestParseVeritySetupStatus_Inactive(t *testing.T) {
	s := ParseVeritySetupStatus("/dev/mapper/root is inactive.", "root")
	if s.Active {
		t.Error("expected Active=false for inactive device")
	}
}
