package scenario

import (
	"testing"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"
)

func TestDistroDefaults(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte("storage: {}\n"))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name   string
		params TridentE2EHostConfigParams
		distro string
	}{
		{"legacy", TridentE2EHostConfigParams{}, "azl3"},
		{"azl4", TridentE2EHostConfigParams{Distro: "azl4"}, "azl4"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, err := NewTridentE2EScenario("test", nil, hc, tc.params,
				HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingSet{testrings.TestRingPrE2e})
			if err != nil {
				t.Fatal(err)
			}
			if s.Distro() != tc.distro {
				t.Fatalf("unexpected distro: got %s, want %s", s.Distro(), tc.distro)
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
		{"azl4-container", TridentE2EHostConfigParams{Distro: "azl4"}, HardwareTypeVM, trident.RuntimeTypeContainer},
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
