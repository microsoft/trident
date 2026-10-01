package hostconfig

import "testing"

// storage.raid.software is optional, so a raid object carrying only
// syncTimeout is a valid configuration with nothing to rebuild. Gating on the
// raid object alone registered the rebuild cases for it, which then failed
// hunting for a RAID member that was never configured.
func TestHasRebuildableRaidRequiresASoftwareArray(t *testing.T) {
	tests := []struct {
		name string
		yaml string
		want bool
	}{
		{"no raid at all", "storage:\n  disks: []\n", false},
		{"raid with only syncTimeout", "storage:\n  raid:\n    syncTimeout: 180\n", false},
		{"raid with an empty software list", "storage:\n  raid:\n    software: []\n", false},
		{"raid with a software array", "storage:\n  raid:\n    software:\n      - id: root\n        devices: [a, b]\n", true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			hc, err := NewHostConfigFromYaml([]byte(tt.yaml))
			if err != nil {
				t.Fatalf("parse: %v", err)
			}
			if got := hc.HasRebuildableRaid(); got != tt.want {
				t.Errorf("HasRebuildableRaid() = %v, want %v", got, tt.want)
			}
		})
	}
}
