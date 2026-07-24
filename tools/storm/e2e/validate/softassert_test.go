package validate

import (
	"errors"
	"strings"
	"testing"
)

func TestSoftAsserter_AllPass(t *testing.T) {
	var sa SoftAsserter
	ran := 0
	sa.Check("a", func() error { ran++; return nil })
	sa.Check("b", func() error { ran++; return nil })
	sa.Assert("c", true, "should not fire")

	if ran != 2 {
		t.Errorf("ran = %d, want 2", ran)
	}
	if sa.HasFailures() {
		t.Error("HasFailures() = true, want false")
	}
	if sa.Err() != nil {
		t.Errorf("Err() = %v, want nil", sa.Err())
	}
}

func TestSoftAsserter_ContinuesAfterFailure(t *testing.T) {
	var sa SoftAsserter
	ran := 0
	sa.Check("first", func() error { ran++; return errors.New("boom") })
	sa.Check("second", func() error { ran++; return nil }) // must still run
	sa.Failf("third", "value %d bad", 7)
	sa.Assert("fourth", false, "cond false")

	if ran != 2 {
		t.Errorf("ran = %d, want 2 (both Check fns must execute)", ran)
	}
	if !sa.HasFailures() {
		t.Fatal("HasFailures() = false, want true")
	}
	if sa.Failures() != 3 {
		t.Errorf("Failures() = %d, want 3", sa.Failures())
	}

	err := sa.Err()
	if err == nil {
		t.Fatal("Err() = nil, want combined error")
	}
	msg := err.Error()
	for _, want := range []string{"first: boom", "third: value 7 bad", "fourth: cond false", "3 validation sub-check(s) failed"} {
		if !strings.Contains(msg, want) {
			t.Errorf("Err() = %q, missing %q", msg, want)
		}
	}
}
