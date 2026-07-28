package scenario

import (
	"testing"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/trident"
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

func newEncryptedUsrverityScenario(t *testing.T, runtime trident.RuntimeType) *TridentE2EScenario {
	t.Helper()
	yaml := "image:\n  url: http://x/usrverity.cosi\n" +
		"storage:\n  encryption:\n    pcrs:\n" +
		"      - boot-loader-code\n      - secure-boot-policy\n      - kernel-boot\n" +
		"os:\n  users: []\n"
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(yaml))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	return &TridentE2EScenario{config: hc, runtime: runtime}
}

func pcrList(t *testing.T, s *TridentE2EScenario) []string {
	t.Helper()
	var out []string
	for _, c := range s.config.S("storage", "encryption", "pcrs").Children() {
		if v, ok := c.Data().(string); ok {
			out = append(out, v)
		}
	}
	return out
}

func TestApplyContainerPcrExclusion_StripsPcr7(t *testing.T) {
	s := newEncryptedUsrverityScenario(t, trident.RuntimeTypeContainer)
	s.applyContainerPcrExclusion()

	got := pcrList(t, s)
	want := []string{"boot-loader-code", "kernel-boot"}
	if len(got) != len(want) {
		t.Fatalf("pcrs = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("pcrs = %v, want %v (PCR 7 must be dropped)", got, want)
		}
	}
}

func TestApplyContainerPcrExclusion_HostUnchanged(t *testing.T) {
	s := newEncryptedUsrverityScenario(t, trident.RuntimeTypeHost)
	s.applyContainerPcrExclusion()

	if got := len(pcrList(t, s)); got != 3 {
		t.Errorf("host pcrs len = %d, want 3 (unchanged)", got)
	}
}

func TestApplyContainerPcrExclusion_NonUsrverityUnchanged(t *testing.T) {
	// A grub/regular image in a container keeps its PCRs (the constraint is
	// specific to usr-verity UKI images).
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(
		"image:\n  url: http://x/regular.cosi\n" +
			"storage:\n  encryption:\n    pcrs:\n      - secure-boot-policy\n" +
			"os:\n  users: []\n"))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	s := &TridentE2EScenario{config: hc, runtime: trident.RuntimeTypeContainer}
	s.applyContainerPcrExclusion()

	got := pcrList(t, s)
	if len(got) != 1 || got[0] != "secure-boot-policy" {
		t.Errorf("non-usrverity pcrs = %v, want [secure-boot-policy] (unchanged)", got)
	}
}
