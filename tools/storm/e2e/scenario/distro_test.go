package scenario

import (
	"testing"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"
)

func TestDistroAndSecureBootDefaults(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte("storage: {}\n"))
	if err != nil {
		t.Fatal(err)
	}
	disabled := false
	for _, tc := range []struct {
		name       string
		params     TridentE2EHostConfigParams
		distro     string
		secureBoot bool
	}{
		{"legacy", TridentE2EHostConfigParams{}, "azl3", true},
		{"azl4", TridentE2EHostConfigParams{Distro: "azl4", SecureBoot: &disabled}, "azl4", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, err := NewTridentE2EScenario("test", nil, hc, tc.params,
				HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingSet{testrings.TestRingPrE2e})
			if err != nil {
				t.Fatal(err)
			}
			if s.Distro() != tc.distro || s.secureBootEnabled() != tc.secureBoot {
				t.Fatalf("unexpected distro/Secure Boot: %s/%t", s.Distro(), s.secureBootEnabled())
			}
		})
	}
}

func TestRejectUnsupportedDistroCoverage(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte("storage: {}\n"))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name     string
		params   TridentE2EHostConfigParams
		hardware HardwareType
		runtime  trident.RuntimeType
	}{
		{"unknown", TridentE2EHostConfigParams{Distro: "typo"}, HardwareTypeVM, trident.RuntimeTypeHost},
		{"azl4-container-uki", TridentE2EHostConfigParams{Distro: "azl4", IsUki: true}, HardwareTypeVM, trident.RuntimeTypeContainer},
		{"azl4-baremetal", TridentE2EHostConfigParams{Distro: "azl4"}, HardwareTypeBM, trident.RuntimeTypeHost},
		{"azl4-uki", TridentE2EHostConfigParams{Distro: "azl4", IsUki: true}, HardwareTypeVM, trident.RuntimeTypeHost},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := NewTridentE2EScenario("test", nil, hc, tc.params,
				tc.hardware, tc.runtime, nil); err == nil {
				t.Fatal("unsupported distro coverage accepted")
			}
		})
	}
}

func TestAzl4GrubVmContainerAccepted(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte("storage: {}\n"))
	if err != nil {
		t.Fatal(err)
	}
	disabled := false
	s, err := NewTridentE2EScenario("root-verity-azl4", nil, hc,
		TridentE2EHostConfigParams{Distro: "azl4", SecureBoot: &disabled},
		HardwareTypeVM, trident.RuntimeTypeContainer, nil)
	if err != nil {
		t.Fatal(err)
	}
	if s.Distro() != "azl4" || s.RuntimeType() != trident.RuntimeTypeContainer || s.secureBootEnabled() {
		t.Fatal("incorrect AZL4 GRUB container defaults")
	}
}
