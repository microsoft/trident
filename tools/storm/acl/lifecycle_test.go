package acl

import (
	"errors"
	"fmt"
	"slices"
	"testing"
)

type lifecycleGuest struct {
	commands, slots []string
	boots           int
	commandErr      error
	checkErr        error
}

func (g *lifecycleGuest) command(command string) (string, error) {
	g.commands = append(g.commands, command)
	return "", g.commandErr
}

func (g *lifecycleGuest) trident(string, string, bool) (string, error) { return "", nil }
func (g *lifecycleGuest) reboot() error                                { return nil }

func (g *lifecycleGuest) check(_, slot string) error {
	g.slots = append(g.slots, slot)
	return g.checkErr
}

func (g *lifecycleGuest) waitBoot(string) (string, error) {
	g.boots++
	return fmt.Sprintf("boot-%d", g.boots), nil
}

func TestCheckDeploymentValidatesBaseline(t *testing.T) {
	failure := errors.New("baseline validation failed")
	for _, tc := range []struct {
		name    string
		mode    deploySlotValidation
		slot    string
		failure error
	}{
		{"fresh", validationNoDatastore, "a", nil},
		{"datastore exists or SSH failed", validationNoDatastore, "a", failure},
		{"adopted", validationCheckExpected, "b", nil},
		{"unexpected status", validationCheckExpected, "b", failure},
	} {
		t.Run(tc.name, func(t *testing.T) {
			g := &lifecycleGuest{}
			if tc.mode == validationNoDatastore {
				g.commandErr = tc.failure
			} else {
				g.checkErr = tc.failure
			}
			l := lifecycle{g: g, bootID: "previous", currentSlot: tc.slot}
			err := l.checkDeployment(tc.mode)
			if !errors.Is(err, tc.failure) {
				t.Fatalf("error = %v, want %v", err, tc.failure)
			}
			if tc.mode == validationNoDatastore {
				if len(g.commands) != 1 || len(g.slots) != 0 {
					t.Fatalf("fresh baseline commands = %v, status checks = %v", g.commands, g.slots)
				}
			} else if len(g.commands) != 0 || !slices.Equal(g.slots, []string{tc.slot}) {
				t.Fatalf("adopted baseline commands = %v, status checks = %v", g.commands, g.slots)
			}
			wantBoot := "boot-1"
			if tc.failure != nil {
				wantBoot = "previous"
			}
			if l.bootID != wantBoot || l.currentSlot != tc.slot {
				t.Fatalf("baseline state = (%s, %s), want (%s, %s)", l.bootID, l.currentSlot, wantBoot, tc.slot)
			}
		})
	}
}

func TestLifecycleTransitionsFromConfiguredSlot(t *testing.T) {
	for _, tc := range []struct {
		initial string
		mode    deploySlotValidation
		want    []string
	}{
		{"a", validationNoDatastore, []string{"b", "a", "b", "a", "a"}},
		{"b", validationCheckExpected, []string{"a", "b", "a", "b", "b"}},
	} {
		t.Run(tc.initial, func(t *testing.T) {
			g := &lifecycleGuest{}
			l := lifecycle{g: g, currentSlot: tc.initial}
			if err := l.checkDeployment(tc.mode); err != nil {
				t.Fatal(err)
			}
			g.slots = nil
			for i, rollback := range []bool{false, true, false, false} {
				if err := l.update(fmt.Sprintf("operation-%d", i), rollback); err != nil {
					t.Fatal(err)
				}
			}
			if err := l.checkPersistence(); err != nil {
				t.Fatal(err)
			}
			if !slices.Equal(g.slots, tc.want) {
				t.Fatalf("validated slots = %v, want %v", g.slots, tc.want)
			}
		})
	}
}

func TestFailedSlotCheckPreservesAcceptedState(t *testing.T) {
	failure := errors.New("unexpected active slot")
	g := &lifecycleGuest{checkErr: failure}
	l := lifecycle{g: g, bootID: "accepted-boot", currentSlot: "b"}
	if err := l.update("update", false); !errors.Is(err, failure) {
		t.Fatalf("expected status failure, got %v", err)
	}
	if !slices.Equal(g.slots, []string{"a"}) || l.currentSlot != "b" || l.bootID != "accepted-boot" {
		t.Fatalf("failed transition changed accepted state: checks=%v, slot=%s, boot=%s", g.slots, l.currentSlot, l.bootID)
	}
}
