package acl

import (
	"fmt"
	"strings"
)

type checkpoint struct {
	Phase, Slot, Generation, Previous, State string
}

type guest interface {
	command(string) (string, error)
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
	sum, err := g.command("sha256sum " + guestDir + "/update.cosi")
	if err != nil {
		return err
	}
	fields := strings.Fields(sum)
	if len(fields) != 2 || fields[0] != info.CosiSHA256 {
		return fmt.Errorf("uploaded COSI SHA256 mismatch: %q", sum)
	}
	if err := g.check(checkpoint{Phase: "baseline", Slot: "a", Generation: "base"}, info); err != nil {
		return fmt.Errorf("baseline: %w", err)
	}
	previous, activeSlot, generation := "baseline", "a", "base"
	for _, step := range []struct {
		name, slot, generation string
		rollback               bool
	}{
		{name: "update-1", slot: "b", generation: "update"},
		{name: "rollback", slot: "a", generation: "base", rollback: true},
		{name: "update-2", slot: "b", generation: "update"},
		{name: "update-3", slot: "a", generation: "update"},
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
		if err := g.check(checkpoint{staged, activeSlot, generation, previous, stagedState}, info); err != nil {
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
		if err := g.check(checkpoint{step.name, step.slot, step.generation, staged, "provisioned"}, info); err != nil {
			return fmt.Errorf("%s: %w", step.name, err)
		}
		previous, activeSlot, generation = step.name, step.slot, step.generation
	}
	if err := g.reboot(); err != nil {
		return fmt.Errorf("persisted reboot: %w", err)
	}
	if _, err := g.waitBoot(bootID); err != nil {
		return fmt.Errorf("persisted boot: %w", err)
	}
	if err := g.check(checkpoint{"persisted", activeSlot, generation, previous, "provisioned"}, info); err != nil {
		return fmt.Errorf("persisted: %w", err)
	}
	return nil
}
