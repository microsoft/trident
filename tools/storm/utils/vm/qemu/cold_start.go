package qemu

import (
	"context"
	"fmt"
	"time"

	"github.com/digitalocean/go-libvirt"
)

// domainStarter exposes the libvirt operations needed to wait for shutoff and start a domain.
type domainStarter interface {
	DomainGetState(libvirt.Domain, uint32) (int32, int32, error)
	DomainCreate(libvirt.Domain) error
}

// startDomainWhenShutoff waits for a stopped domain before starting it without redefining it.
func startDomainWhenShutoff(ctx context.Context, lv domainStarter, domain libvirt.Domain) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		state, reason, err := lv.DomainGetState(domain, 0)
		if err != nil {
			return err
		}
		if state == int32(libvirt.DomainShutoff) {
			break
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("waiting for shutoff (state %d, reason %d): %w", state, reason, ctx.Err())
		case <-ticker.C:
		}
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	return lv.DomainCreate(domain)
}

// StartQemuVMWhenShutoff waits for guest-initiated shutdown without forcing it or replacing NVRAM.
func (cfg QemuConfig) StartQemuVMWhenShutoff(ctx context.Context, vmName string) error {
	lv, domain, err := getLibvirtDomainByname(vmName)
	if err != nil {
		return err
	}
	defer lv.Disconnect()
	if err := startDomainWhenShutoff(ctx, lv, domain); err != nil {
		return fmt.Errorf("starting domain %q after guest shutdown: %w", vmName, err)
	}
	return nil
}

// domainColdRestarter adds forced stop support while retaining the domain definition.
type domainColdRestarter interface {
	domainStarter
	DomainDestroy(libvirt.Domain) error
}

// coldRestartDomain stops a running domain, then starts it once shutoff is observed.
func coldRestartDomain(ctx context.Context, lv domainColdRestarter, domain libvirt.Domain) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	state, reason, err := lv.DomainGetState(domain, 0)
	if err != nil {
		return err
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	switch libvirt.DomainState(state) {
	case libvirt.DomainRunning:
		if err := lv.DomainDestroy(domain); err != nil {
			return err
		}
	case libvirt.DomainShutoff:
	default:
		return fmt.Errorf("cannot cold-restart domain in state %d (reason %d)", state, reason)
	}
	return startDomainWhenShutoff(ctx, lv, domain)
}

// ColdRestartQemuVM stops and starts the existing domain, preserving its disk and NVRAM.
// The ACL MSHV caller must first confirm Linux reached its final restart boundary.
func (cfg QemuConfig) ColdRestartQemuVM(ctx context.Context, vmName string) error {
	lv, domain, err := getLibvirtDomainByname(vmName)
	if err != nil {
		return err
	}
	defer lv.Disconnect()
	stop := context.AfterFunc(ctx, func() { _ = lv.Disconnect() })
	defer stop()
	if err := coldRestartDomain(ctx, lv, domain); err != nil {
		return fmt.Errorf("cold-restarting domain %q after guest restart: %w", vmName, err)
	}
	return nil
}
