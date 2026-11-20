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

type lifecycle struct {
	g                            guest
	info                         artifactInfo
	bootID, previous, activeSlot string
}

func (l *lifecycle) checkDeployment() error {
	bootID, err := l.g.waitBoot("")
	if err != nil {
		return fmt.Errorf("baseline SSH: %w", err)
	}
	if err := l.g.check(checkpoint{Phase: "baseline", Slot: "a"}, l.info); err != nil {
		return fmt.Errorf("baseline: %w", err)
	}
	l.bootID, l.previous, l.activeSlot = bootID, "baseline", "a"
	return nil
}

func (l *lifecycle) update(name, slot string, rollback bool) error {
	command := "update " + guestDir + "/update.yaml"
	stagedState := "ab-update-staged"
	if rollback {
		command, stagedState = "rollback --ab", "manual-rollback-ab-staged"
	}
	if _, err := l.g.trident(command+" --allowed-operations stage", name+"-stage", false); err != nil {
		return fmt.Errorf("%s stage: %w", name, err)
	}
	staged := name + "-staged"
	if err := l.g.check(checkpoint{staged, l.activeSlot, l.previous, stagedState}, l.info); err != nil {
		return fmt.Errorf("%s: %w", staged, err)
	}
	// Native rollback restores a derived config without noTransition.
	if _, err := l.g.trident(command+" --allowed-operations finalize", name+"-finalize", rollback); err != nil {
		return fmt.Errorf("%s finalize: %w", name, err)
	}
	if !rollback {
		if err := l.g.reboot(); err != nil {
			return fmt.Errorf("%s reboot: %w", name, err)
		}
	}
	bootID, err := l.g.waitBoot(l.bootID)
	if err != nil {
		return fmt.Errorf("%s boot: %w", name, err)
	}
	if _, err := l.g.trident("commit", name+"-commit", false); err != nil {
		return fmt.Errorf("%s commit: %w", name, err)
	}
	if err := l.g.check(checkpoint{name, slot, staged, "provisioned"}, l.info); err != nil {
		return fmt.Errorf("%s: %w", name, err)
	}
	l.bootID, l.previous, l.activeSlot = bootID, name, slot
	return nil
}

func (l *lifecycle) checkPersistence() error {
	if err := l.g.reboot(); err != nil {
		return fmt.Errorf("persisted reboot: %w", err)
	}
	if _, err := l.g.waitBoot(l.bootID); err != nil {
		return fmt.Errorf("persisted boot: %w", err)
	}
	if err := l.g.check(checkpoint{"persisted", l.activeSlot, l.previous, "provisioned"}, l.info); err != nil {
		return fmt.Errorf("persisted: %w", err)
	}
	return nil
}
