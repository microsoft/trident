package acl

import "fmt"

type checkpoint struct {
	Phase, Slot, Previous, State string
}

type guest interface {
	trident(arguments, logName string, expectReboot bool) (string, error)
	check(checkpoint, artifactInfo) error
	waitBoot(string) (string, error)
	reboot() error
}

func service(g guest, info artifactInfo) error {
	bootID, err := g.waitBoot("")
	if err != nil {
		return fmt.Errorf("baseline SSH: %w", err)
	}
	if err := g.check(checkpoint{Phase: "baseline", Slot: "a"}, info); err != nil {
		return fmt.Errorf("baseline: %w", err)
	}
	previous, activeSlot := "baseline", "a"
	for _, step := range []struct {
		name, slot string
		rollback   bool
	}{
		{name: "update-1", slot: "b"},
		{name: "rollback", slot: "a", rollback: true},
		{name: "update-2", slot: "b"},
		{name: "update-3", slot: "a"},
	} {
		command := "update " + guestDir + "/update.yaml"
		stagedState := "ab-update-staged"
		if step.rollback {
			command, stagedState = "rollback --ab", "manual-rollback-ab-staged"
		}
		if _, err := g.trident(command+" --allowed-operations stage", step.name+"-stage", false); err != nil {
			return fmt.Errorf("%s stage: %w", step.name, err)
		}
		staged := step.name + "-staged"
		if err := g.check(checkpoint{staged, activeSlot, previous, stagedState}, info); err != nil {
			return fmt.Errorf("%s: %w", staged, err)
		}
		// Native rollback restores a derived config without noTransition.
		if _, err := g.trident(command+" --allowed-operations finalize", step.name+"-finalize", step.rollback); err != nil {
			return fmt.Errorf("%s finalize: %w", step.name, err)
		}
		if !step.rollback {
			if err := g.reboot(); err != nil {
				return fmt.Errorf("%s reboot: %w", step.name, err)
			}
		}
		bootID, err = g.waitBoot(bootID)
		if err != nil {
			return fmt.Errorf("%s boot: %w", step.name, err)
		}
		if _, err := g.trident("commit", step.name+"-commit", false); err != nil {
			return fmt.Errorf("%s commit: %w", step.name, err)
		}
		if err := g.check(checkpoint{step.name, step.slot, staged, "provisioned"}, info); err != nil {
			return fmt.Errorf("%s: %w", step.name, err)
		}
		previous, activeSlot = step.name, step.slot
	}
	if err := g.reboot(); err != nil {
		return fmt.Errorf("persisted reboot: %w", err)
	}
	if _, err := g.waitBoot(bootID); err != nil {
		return fmt.Errorf("persisted boot: %w", err)
	}
	if err := g.check(checkpoint{"persisted", activeSlot, previous, "provisioned"}, info); err != nil {
		return fmt.Errorf("persisted: %w", err)
	}
	return nil
}
