package scenario

import (
	"math"
	"testing"
)

func TestParseSizeToBytes(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected float64
		wantErr  bool
	}{
		{"bytes numeric", "1024", 1024, false},
		{"bytes unit", "512B", 512, false},
		{"kilobytes", "1K", 1024, false},
		{"megabytes", "512M", 512 * math.Pow(1024, 2), false},
		{"gigabytes", "8G", 8 * math.Pow(1024, 3), false},
		{"terabytes", "1T", math.Pow(1024, 4), false},
		{"petabytes", "1P", math.Pow(1024, 5), false},
		{"fractional", "1.5G", 1.5 * math.Pow(1024, 3), false},
		{"lowercase", "8g", 8 * math.Pow(1024, 3), false},
		{"empty string", "", 0, true},
		{"invalid unit", "8X", 0, true},
		{"invalid number", "abcG", 0, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := parseSizeToBytes(tt.input)
			if tt.wantErr {
				if err == nil {
					t.Errorf("parseSizeToBytes(%q) expected error, got nil", tt.input)
				}
				return
			}
			if err != nil {
				t.Errorf("parseSizeToBytes(%q) unexpected error: %v", tt.input, err)
				return
			}
			if result != tt.expected {
				t.Errorf("parseSizeToBytes(%q) = %v, want %v", tt.input, result, tt.expected)
			}
		})
	}
}
