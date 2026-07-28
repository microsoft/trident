package scenario

import (
	"testing"

	"tridenttools/pkg/hostconfig"
)

func newInjectScenario(t *testing.T) *TridentE2EScenario {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte("image:\n  url: http://x/regular.cosi\nos:\n  users: []\n"))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	return &TridentE2EScenario{config: hc}
}

func TestApplyOciOverrides_Sysext(t *testing.T) {
	s := newInjectScenario(t)
	s.args.SysextOciUrl = "oci://acr.example/sysext-host:1.2.3.1"
	s.args.SysextSha384 = "abc123"
	s.applyOciOverrides()

	sysexts := s.config.S("os", "sysexts").Children()
	if len(sysexts) != 1 {
		t.Fatalf("got %d sysexts, want 1", len(sysexts))
	}
	if got := sysexts[0].S("url").Data(); got != "oci://acr.example/sysext-host:1.2.3.1" {
		t.Errorf("sysext url = %v", got)
	}
	if got := sysexts[0].S("sha384").Data(); got != "abc123" {
		t.Errorf("sysext sha384 = %v", got)
	}
}

func TestApplyOciOverrides_Confext(t *testing.T) {
	s := newInjectScenario(t)
	s.args.ConfextOciUrl = "oci://acr.example/confext:1"
	s.args.ConfextSha384 = "def456"
	s.applyOciOverrides()

	confexts := s.config.S("os", "confexts").Children()
	if len(confexts) != 1 {
		t.Fatalf("got %d confexts, want 1", len(confexts))
	}
	if got := confexts[0].S("sha384").Data(); got != "def456" {
		t.Errorf("confext sha384 = %v", got)
	}
}

func TestApplyOciOverrides_ImageUrl(t *testing.T) {
	s := newInjectScenario(t)
	s.args.OciImageUrl = "oci://acr.example/trident-testimage:1.2.3.1"
	s.applyOciOverrides()

	if got := s.config.S("image", "url").Data(); got != "oci://acr.example/trident-testimage:1.2.3.1" {
		t.Errorf("image url = %v, want the OCI override", got)
	}
}

func TestApplyOciOverrides_NoneSet(t *testing.T) {
	s := newInjectScenario(t)
	s.applyOciOverrides()

	if s.config.Exists("os", "sysexts") {
		t.Error("os.sysexts should not be created when no arg set")
	}
	if s.config.Exists("os", "confexts") {
		t.Error("os.confexts should not be created when no arg set")
	}
	if got := s.config.S("image", "url").Data(); got != "http://x/regular.cosi" {
		t.Errorf("image url should be unchanged, got %v", got)
	}
}
