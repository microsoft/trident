package activevolume

import "testing"

func TestValidateHostStatus(t *testing.T) {
	for _, tt := range []struct {
		name, status, expected string
		valid                  bool
	}{
		{"provisioned a", "servicingState: provisioned\nabActiveVolume: volume-a\n", "volume-a", true},
		{"provisioned b", "servicingState: provisioned\nabActiveVolume: volume-b\n", "volume-b", true},
		{"uncommitted", "servicingState: ab-update-finalized\nabActiveVolume: volume-b\n", "volume-b", false},
		{"wrong volume", "servicingState: provisioned\nabActiveVolume: volume-a\n", "volume-b", false},
		{"missing volume", "servicingState: provisioned\n", "volume-a", false},
		{"invalid yaml", "servicingState: [", "volume-a", false},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if err := ValidateHostStatus(tt.status, tt.expected); (err == nil) != tt.valid {
				t.Fatalf("valid=%v, got %v", tt.valid, err)
			}
		})
	}
}
