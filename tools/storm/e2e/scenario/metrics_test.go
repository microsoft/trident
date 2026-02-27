package scenario

import (
	"testing"
)

func TestParseBootTiming(t *testing.T) {
	tests := []struct {
		name      string
		input     string
		target    string
		wantVal   string
		wantUnit  string
		wantFound bool
	}{
		{
			name:      "firmware in seconds",
			input:     "Startup finished in 13.022s (firmware) + 2.552s (loader) + 4.740s (kernel)",
			target:    "(firmware)",
			wantVal:   "13.022",
			wantUnit:  "s",
			wantFound: true,
		},
		{
			name:      "loader in seconds",
			input:     "Startup finished in 13.022s (firmware) + 2.552s (loader) + 4.740s (kernel)",
			target:    "(loader)",
			wantVal:   "2.552",
			wantUnit:  "s",
			wantFound: true,
		},
		{
			name:      "kernel in seconds",
			input:     "Startup finished in 4.740s (kernel) + 1.267s (initrd) + 15.249s (userspace) = 35.565s",
			target:    "(kernel)",
			wantVal:   "4.740",
			wantUnit:  "s",
			wantFound: true,
		},
		{
			name:      "not found",
			input:     "Startup finished in 4.740s (kernel)",
			target:    "(firmware)",
			wantFound: false,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			val, unit, found := parseBootTiming(tc.input, tc.target)
			if found != tc.wantFound {
				t.Errorf("found=%v, want %v", found, tc.wantFound)
			}
			if found {
				if val != tc.wantVal {
					t.Errorf("val=%q, want %q", val, tc.wantVal)
				}
				if unit != tc.wantUnit {
					t.Errorf("unit=%q, want %q", unit, tc.wantUnit)
				}
			}
		})
	}
}

func TestToMilliseconds(t *testing.T) {
	tests := []struct {
		value string
		unit  string
		want  float64
		err   bool
	}{
		{"4.740", "s", 4740, false},
		{"100", "ms", 100, false},
		{"2", "m", 120000, false},
		{"500000", "ns", 0.5, false},
		{"1.5", "x", 0, true},
	}

	for _, tc := range tests {
		t.Run(tc.value+tc.unit, func(t *testing.T) {
			got, err := toMilliseconds(tc.value, tc.unit)
			if (err != nil) != tc.err {
				t.Errorf("err=%v, wantErr=%v", err, tc.err)
			}
			if !tc.err && got != tc.want {
				t.Errorf("got=%f, want=%f", got, tc.want)
			}
		})
	}
}
