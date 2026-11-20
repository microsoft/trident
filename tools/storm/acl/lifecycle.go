package acl

import "fmt"

// guest supplies the operations needed to drive servicing independently of VM transport.
type guest interface {
	trident(arguments, logName string, expectReboot bool) (string, error)
	check(phase, slot string) error
	waitBoot(string) (string, error)
	reboot() error
}

// lifecycle tracks the last accepted boot and active slot across servicing cases.
type lifecycle struct {
	g                  guest
	bootID, activeSlot string
}

// checkDeployment establishes SSH readiness and the native image's initial A slot.
func (l *lifecycle) checkDeployment() error {
	bootID, err := l.g.waitBoot(NO_PREVIOUS_BOOT_ID)
	if err != nil {
		return fmt.Errorf("baseline SSH: %w", err)
	}
	l.bootID, l.activeSlot = bootID, "a"
	return nil
}

// update stages, finalizes and commits an update or rollback to the expected slot.
func (l *lifecycle) update(name, slot string, rollback bool) error {
	command := "update " + guestDir + "/update.yaml"
	if rollback {
		command = "rollback --ab"
	}
	if _, err := l.g.trident(command+" --allowed-operations stage", name+"-stage", false); err != nil {
		return fmt.Errorf("%s stage: %w", name, err)
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
	if err := l.g.check(name, slot); err != nil {
		return fmt.Errorf("%s: %w", name, err)
	}
	l.bootID, l.activeSlot = bootID, slot
	return nil
}

// checkPersistence verifies the committed slot remains Provisioned after another reboot.
func (l *lifecycle) checkPersistence() error {
	if err := l.g.reboot(); err != nil {
		return fmt.Errorf("persisted reboot: %w", err)
	}
	if _, err := l.g.waitBoot(l.bootID); err != nil {
		return fmt.Errorf("persisted boot: %w", err)
	}
	if err := l.g.check("persisted", l.activeSlot); err != nil {
		return fmt.Errorf("persisted: %w", err)
	}
	return nil
}
