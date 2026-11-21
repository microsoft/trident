package acl

import (
	"testing"

	"github.com/alecthomas/kong"
)

func TestBaselineOptions(t *testing.T) {
	for _, tc := range []struct {
		name    string
		flags   []string
		slot    string
		mode    deploySlotValidation
		invalid bool
	}{
		{"defaults", nil, "a", validationNoDatastore, false},
		{"explicit native", []string{"--deploy-slot-validation", "NoDatastore"}, "a", validationNoDatastore, false},
		{"adopted B", []string{"--expected-first-slot", "b", "--deploy-slot-validation", "CheckExpected"}, "b", validationCheckExpected, false},
		{"invalid slot", []string{"--expected-first-slot", "c"}, "", "", true},
		{"invalid validation", []string{"--deploy-slot-validation", "Other"}, "", "", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var args scenarioArgs
			parser, err := kong.New(&args)
			if err != nil {
				t.Fatal(err)
			}
			flags := append([]string{"--artifacts-dir", "inputs", "--output-path", "results"}, tc.flags...)
			_, err = parser.Parse(flags)
			if tc.invalid {
				if err == nil {
					t.Fatal("invalid baseline option was accepted")
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if args.ExpectedFirstSlot != tc.slot || args.DeploySlotValidation != tc.mode {
				t.Fatalf("baseline options = (%s, %s), want (%s, %s)", args.ExpectedFirstSlot, args.DeploySlotValidation, tc.slot, tc.mode)
			}
		})
	}
}
