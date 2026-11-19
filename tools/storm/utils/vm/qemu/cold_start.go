package qemu

import (
	"context"
	"fmt"
	"time"

	"github.com/digitalocean/go-libvirt"
)

type domainStarter interface {
	DomainGetState(libvirt.Domain, uint32) (int32, int32, error)
	DomainCreate(libvirt.Domain) error
}

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
