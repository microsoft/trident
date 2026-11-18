package acl

import (
	"errors"
	"fmt"
	"slices"
	"strings"
	"testing"
)

type lifecycleGuest struct {
	calls       []string
	checkpoints []checkpoint
	fail        string
	boots       int
}

func (g *lifecycleGuest) record(call string) error {
	g.calls = append(g.calls, call)
	if call == g.fail {
		return errors.New("injected failure")
	}
	return nil
}
func (g *lifecycleGuest) command(command string) (string, error) {
	return strings.Repeat("a", 64) + "  update.cosi", g.record("checksum")
}
func (g *lifecycleGuest) trident(arguments, logName string, reboot bool) (string, error) {
	return "", g.record(fmt.Sprintf("%s:%s:reboot=%t", logName, arguments, reboot))
}
func (g *lifecycleGuest) check(expected checkpoint, _ artifactInfo) error {
	g.checkpoints = append(g.checkpoints, expected)
	return g.record("check:" + expected.Phase)
}
func (g *lifecycleGuest) waitBoot(previous string) (string, error) {
	g.boots++
	return fmt.Sprintf("boot-%d", g.boots), g.record("boot:" + previous)
}
func (g *lifecycleGuest) reboot() error { return g.record("reboot") }

func TestGoLifecycleStagesFinalizesCommitsAndValidatesEachSlot(t *testing.T) {
	g := &lifecycleGuest{}
	if err := service(g, artifactInfo{CosiSHA256: strings.Repeat("a", 64)}); err != nil {
		t.Fatal(err)
	}
	expected := []string{"boot:", "checksum", "check:baseline"}
	for index, step := range []string{"update-1", "rollback", "update-2", "update-3"} {
		command := "update " + guestDir + "/update.yaml"
		rollback := step == "rollback"
		if rollback {
			command = "rollback --ab"
		}
		expected = append(expected,
			fmt.Sprintf("%s-stage:%s --allowed-operations stage:reboot=false", step, command),
			"check:"+step+"-staged",
			fmt.Sprintf("%s-finalize:%s --allowed-operations finalize:reboot=%t", step, command, rollback))
		if !rollback {
			expected = append(expected, "reboot")
		}
		expected = append(expected, fmt.Sprintf("boot:boot-%d", index+1),
			step+"-commit:commit:reboot=false", "check:"+step)
	}
	expected = append(expected, "reboot", "boot:boot-5", "check:persisted")
	if !slices.Equal(g.calls, expected) {
		t.Fatalf("calls:\n%v\nexpected:\n%v", g.calls, expected)
	}
	if g.checkpoints[0].State != "" {
		t.Fatal("baseline must not query nonexistent datastore")
	}
	var committed []string
	for _, check := range g.checkpoints {
		if check.State == "provisioned" {
			committed = append(committed, check.Slot+"/"+check.Generation)
		}
	}
	if !slices.Equal(committed, []string{"b/update", "a/base", "b/update", "a/update", "a/update"}) {
		t.Fatalf("committed checkpoints: %v", committed)
	}
	if g.checkpoints[3].State != "manual-rollback-ab-staged" {
		t.Fatalf("rollback checkpoint: %+v", g.checkpoints[3])
	}
}

func TestGoLifecycleStopsAtEveryFailedOperationOrCheckpoint(t *testing.T) {
	info := artifactInfo{CosiSHA256: strings.Repeat("a", 64)}
	success := &lifecycleGuest{}
	if err := service(success, info); err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	for _, call := range success.calls {
		if seen[call] {
			continue
		}
		seen[call] = true
		t.Run(call, func(t *testing.T) {
			g := &lifecycleGuest{fail: call}
			if err := service(g, info); err == nil {
				t.Fatal("failure ignored")
			}
			if g.calls[len(g.calls)-1] != call {
				t.Fatalf("continued after failure: %v", g.calls)
			}
		})
	}
}
