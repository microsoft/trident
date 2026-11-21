package qemu

import (
	"context"
	"errors"
	"fmt"
	"net"

	"github.com/digitalocean/go-libvirt"
	"github.com/digitalocean/go-libvirt/socket/dialers"
)

// withLibvirtConnection closes the socket directly; Disconnect itself waits on an RPC.
func withLibvirtConnection(ctx context.Context, conn net.Conn, operation func(*libvirt.Libvirt) error) error {
	defer conn.Close()
	if err := ctx.Err(); err != nil {
		return err
	}
	stop := context.AfterFunc(ctx, func() { _ = conn.Close() })
	defer stop()
	lv := libvirt.NewWithDialer(dialers.NewAlreadyConnected(conn))
	err := lv.ConnectToURI(libvirt.QEMUSession)
	if err == nil {
		err = operation(lv)
	}
	return errors.Join(err, ctx.Err())
}

// withLibvirtDomain includes connection setup and lookup in the caller's deadline.
func withLibvirtDomain(ctx context.Context, vmName string, operation func(*libvirt.Libvirt, libvirt.Domain) error) error {
	// Match go-libvirt's default local socket while retaining the QEMU session driver.
	const socketPath = "/var/run/libvirt/libvirt-sock"
	conn, err := (&net.Dialer{}).DialContext(ctx, "unix", socketPath)
	if err != nil {
		return fmt.Errorf("connecting to libvirt: %w", err)
	}
	return withLibvirtConnection(ctx, conn, func(lv *libvirt.Libvirt) error {
		domain, err := lv.DomainLookupByName(vmName)
		if err != nil {
			return fmt.Errorf("looking up domain %q: %w", vmName, err)
		}
		return operation(lv, domain)
	})
}
