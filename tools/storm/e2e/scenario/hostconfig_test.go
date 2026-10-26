package scenario

import (
	"encoding/json"
	"reflect"
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

func newUsrverityScenarioWithPcrs(t *testing.T, pcrs string, runtime trident.RuntimeType) *TridentE2EScenario {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(
		"image:\n  url: http://x/usrverity.cosi\n" +
			"storage:\n  encryption:\n    pcrs: " + pcrs + "\n" +
			"os:\n  users: []\n"))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	return &TridentE2EScenario{config: hc, runtime: runtime}
}

func pcrData(t *testing.T, s *TridentE2EScenario) []interface{} {
	t.Helper()
	var out []interface{}
	for _, c := range s.config.S("storage", "encryption", "pcrs").Children() {
		out = append(out, c.Data())
	}
	return out
}

func pcrList(t *testing.T, s *TridentE2EScenario) []string {
	t.Helper()
	var out []string
	for _, c := range pcrData(t, s) {
		if v, ok := c.(string); ok {
			out = append(out, v)
		}
	}
	return out
}

func assertPcrData(t *testing.T, s *TridentE2EScenario, want []interface{}) {
	t.Helper()
	got := pcrData(t, s)
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("pcrs = %#v, want %#v", got, want)
	}
}

func TestHostConfigLoaderDecodesNumericPcrsAsInt(t *testing.T) {
	s := newUsrverityScenarioWithPcrs(t, "[4, 7, 11]", trident.RuntimeTypeContainer)

	got := pcrData(t, s)
	if len(got) != 3 {
		t.Fatalf("pcrs = %#v, want three entries", got)
	}
	if _, ok := got[1].(int); !ok {
		t.Fatalf("numeric PCR decoded as %T (%#v), want int", got[1], got[1])
	}
}

func TestSecureBootPolicyPcrNormalizationHandlesDecodeTypes(t *testing.T) {
	tests := []struct {
		name string
		pcr  interface{}
		want bool
	}{
		{name: "name", pcr: secureBootPolicyPcr, want: true},
		{name: "numeric string", pcr: "7", want: true},
		{name: "int", pcr: int(7), want: true},
		{name: "float64", pcr: float64(7), want: true},
		{name: "json number", pcr: json.Number("7"), want: true},
		{name: "other pcr", pcr: int(11), want: false},
		{name: "other string", pcr: "kernel-boot", want: false},
		{name: "non-integral json number", pcr: json.Number("7.1"), want: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isSecureBootPolicyPcr(tt.pcr); got != tt.want {
				t.Fatalf("isSecureBootPolicyPcr(%#v) = %v, want %v", tt.pcr, got, tt.want)
			}
		})
	}
}

func TestApplyContainerPcrExclusionDropsPcr7Spellings(t *testing.T) {
	tests := []struct {
		name string
		pcrs string
		want []interface{}
	}{
		{
			name: "name spelling",
			pcrs: "[boot-loader-code, secure-boot-policy, kernel-boot]",
			want: []interface{}{"boot-loader-code", "kernel-boot"},
		},
		{
			name: "bare number",
			pcrs: "[4, 7, 11]",
			want: []interface{}{4, 11},
		},
		{
			name: "numeric string",
			pcrs: `["4", "7", "11"]`,
			want: []interface{}{"4", "11"},
		},
		{
			name: "mixed list",
			pcrs: `[boot-loader-code, 7, "secure-boot-policy", "7", kernel-boot]`,
			want: []interface{}{"boot-loader-code", "kernel-boot"},
		},
		{
			name: "no pcr 7",
			pcrs: `[4, "11", kernel-boot]`,
			want: []interface{}{4, "11", "kernel-boot"},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newUsrverityScenarioWithPcrs(t, tt.pcrs, trident.RuntimeTypeContainer)

			if err := s.applyContainerPcrExclusion(); err != nil {
				t.Fatalf("applyContainerPcrExclusion: %v", err)
			}

			assertPcrData(t, s, tt.want)
		})
	}
}

func TestApplyContainerPcrExclusion_StripsPcr7(t *testing.T) {
	s := newEncryptedUsrverityScenario(t, trident.RuntimeTypeContainer)
	if err := s.applyContainerPcrExclusion(); err != nil {
		t.Fatalf("applyContainerPcrExclusion: %v", err)
	}

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
	if err := s.applyContainerPcrExclusion(); err != nil {
		t.Fatalf("applyContainerPcrExclusion: %v", err)
	}

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
	if err := s.applyContainerPcrExclusion(); err != nil {
		t.Fatalf("applyContainerPcrExclusion: %v", err)
	}

	got := pcrList(t, s)
	if len(got) != 1 || got[0] != "secure-boot-policy" {
		t.Errorf("non-usrverity pcrs = %v, want [secure-boot-policy] (unchanged)", got)
	}
}

func TestInjectRollbackHealthChecks(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(
		"image:\n  url: http://x/regular.cosi\n" +
			"storage:\n  disks:\n  - id: os\n" +
			"os:\n  users: []\n"))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	s := &TridentE2EScenario{config: hc}
	if err := s.injectRollbackHealthChecks(nil); err != nil {
		t.Fatalf("injectRollbackHealthChecks: %v", err)
	}

	// Two failing health checks gated to the ab-update phase.
	checks := s.config.S("health", "checks").Children()
	if len(checks) != 2 {
		t.Fatalf("health.checks len = %d, want 2", len(checks))
	}
	names := map[string]bool{}
	for _, c := range checks {
		names[c.S("name").Data().(string)] = true
		runOn := c.S("runOn").Children()
		if len(runOn) != 1 || runOn[0].Data().(string) != "ab-update" {
			t.Errorf("check %v runOn = %v, want [ab-update]", c.S("name").Data(), runOn)
		}
	}
	for _, want := range []string{rollbackScriptCheckName, rollbackSystemdCheckName} {
		if !names[want] {
			t.Errorf("missing health check %q (have %v)", want, names)
		}
	}
}

// Only PCR 7 may be dropped. The hard-coded replacement this used to do
// happened to equal the right answer for today's configurations, but would
// silently discard any additional PCR a configuration sealed to.
func TestApplyContainerPcrExclusionKeepsOtherPcrs(t *testing.T) {
	s := newScenarioForTest(t, `
image:
  url: http://example/regular-usrverity.cosi
storage:
  encryption:
    pcrs:
    - boot-loader-code
    - secure-boot-policy
    - kernel-boot
    - boot-loader-config
`)
	s.runtime = trident.RuntimeTypeContainer

	if err := s.applyContainerPcrExclusion(); err != nil {
		t.Fatalf("applyContainerPcrExclusion: %v", err)
	}

	var got []string
	for _, pcr := range s.config.S("storage", "encryption", "pcrs").Children() {
		got = append(got, pcr.Data().(string))
	}

	want := []string{"boot-loader-code", "kernel-boot", "boot-loader-config"}
	if len(got) != len(want) {
		t.Fatalf("got %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("pcr %d: got %q, want %q", i, got[i], want[i])
		}
	}
}

// The exclusion is specific to a containerized runtime on a usr-verity image.
func TestApplyContainerPcrExclusionLeavesHostRuntimeAlone(t *testing.T) {
	s := newScenarioForTest(t, `
image:
  url: http://example/regular-usrverity.cosi
storage:
  encryption:
    pcrs:
    - secure-boot-policy
`)
	s.runtime = trident.RuntimeTypeHost

	if err := s.applyContainerPcrExclusion(); err != nil {
		t.Fatalf("applyContainerPcrExclusion: %v", err)
	}

	if n := len(s.config.S("storage", "encryption", "pcrs").Children()); n != 1 {
		t.Errorf("host runtime should keep its PCRs, got %d", n)
	}
}

// The encryption policy has to be decided from the image that is actually
// deployed. --oci-image-url can turn a container configuration into a
// usr-verity one, and classifying before applying it left PCR 7 in the policy,
// which Trident then rejects during dynamic validation.
func TestPcrExclusionUsesTheOverriddenImageUrl(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(
		"image:\n  url: http://x/regular.cosi\nstorage:\n  encryption:\n    pcrs: [secure-boot-policy, boot-loader-code]\n"))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	s := &TridentE2EScenario{config: hc, runtime: trident.RuntimeTypeContainer}
	s.args.OciImageUrl = "oci://acr.example/trident-usrverity.cosi"

	// Through the production entry point, so a future reordering is caught.
	if err := s.applyImageOverrides(); err != nil {
		t.Fatalf("applyImageOverrides: %v", err)
	}

	for _, pcr := range s.config.S("storage", "encryption", "pcrs").Children() {
		if name, _ := pcr.Data().(string); name == secureBootPolicyPcr {
			t.Fatalf("PCR 7 retained after the OCI override made the image usr-verity")
		}
	}
}
