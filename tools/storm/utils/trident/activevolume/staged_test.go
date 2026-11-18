package activevolume

import "testing"

func TestValidateStagedHostStatus(t *testing.T) {
	for _, state := range []string{"ab-update-staged", "manual-rollback-ab-staged"} {
		status := "servicingState: " + state + "\nabActiveVolume: volume-b\n"
		if err := ValidateHostStatusState(status, "volume-b", state); err != nil {
			t.Fatal(err)
		}
		if err := ValidateHostStatus(status, "volume-b"); err == nil {
			t.Fatal("staged status accepted as Provisioned")
		}
		if err := ValidateHostStatusState(status, "volume-a", state); err == nil {
			t.Fatal("wrong staged volume accepted")
		}
	}
}
