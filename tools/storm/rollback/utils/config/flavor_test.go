package config

import "testing"

// The expectations below mirror what the pipeline templates passed before the
// flavor mapping moved into Go, so a regression here means CI would start
// exercising a different set of tests than it does today.
func TestFlavorProfile(t *testing.T) {
	tests := []struct {
		flavor Flavor
		want   FlavorProfile
	}{
		{
			flavor: FlavorQemuGrub,
			want: FlavorProfile{
				SupportsSecureBoot: true,
			},
		},
		{
			flavor: FlavorQemu,
			want: FlavorProfile{
				SkipExtensionTesting:      true,
				SkipRuntimeUpdates:        true,
				SkipNetplanRuntimeTesting: true,
				Uki:                       true,
				SupportsSecureBoot:        true,
			},
		},
		{
			flavor: FlavorUki,
			want: FlavorProfile{
				Uki: true,
			},
		},
	}

	for _, tt := range tests {
		t.Run(string(tt.flavor), func(t *testing.T) {
			got, err := tt.flavor.Profile()
			if err != nil {
				t.Fatalf("Profile() returned error: %v", err)
			}
			if got != tt.want {
				t.Errorf("Profile() = %+v, want %+v", got, tt.want)
			}
		})
	}
}

func TestFlavorProfileUnknown(t *testing.T) {
	if _, err := Flavor("nonsense").Profile(); err == nil {
		t.Fatal("Profile() accepted an unknown flavor, want error")
	}
}

func TestApplyFlavorSetsSkips(t *testing.T) {
	cfg := TestConfig{Flavor: string(FlavorQemu)}

	if _, err := cfg.ApplyFlavor(); err != nil {
		t.Fatalf("ApplyFlavor() returned error: %v", err)
	}

	if !cfg.SkipExtensionTesting || !cfg.SkipRuntimeUpdates || !cfg.SkipNetplanRuntimeTesting || !cfg.Uki {
		t.Errorf("ApplyFlavor() did not apply the qemu profile: %+v", cfg)
	}
}

// A flavor may add skips but must never clear one the caller asked for, so
// flags like --skip-manual-rollbacks stay usable on top of any flavor.
func TestApplyFlavorIsAdditive(t *testing.T) {
	cfg := TestConfig{
		Flavor:               string(FlavorQemuGrub),
		SkipExtensionTesting: true,
		SkipManualRollbacks:  true,
	}

	if _, err := cfg.ApplyFlavor(); err != nil {
		t.Fatalf("ApplyFlavor() returned error: %v", err)
	}

	if !cfg.SkipExtensionTesting {
		t.Error("ApplyFlavor() cleared an explicitly requested SkipExtensionTesting")
	}
	if !cfg.SkipManualRollbacks {
		t.Error("ApplyFlavor() cleared an explicitly requested SkipManualRollbacks")
	}
	if cfg.Uki {
		t.Error("ApplyFlavor() set Uki for the qemu-grub flavor")
	}
}

func TestApplyFlavorUnknownFails(t *testing.T) {
	cfg := TestConfig{Flavor: "nonsense"}

	if _, err := cfg.ApplyFlavor(); err == nil {
		t.Fatal("ApplyFlavor() accepted an unknown flavor, want error")
	}
}
