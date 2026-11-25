package virtdeploy

import (
	"context"
	"errors"
	"fmt"
	"net"

	"github.com/digitalocean/go-libvirt"
	"github.com/digitalocean/go-libvirt/socket/dialers"
)

const libvirtSocketPath = "/var/run/libvirt/libvirt-sock"

// ConnectContext bounds dialing and authentication, not the returned connection's lifetime.
// The returned closer closes the socket without waiting for a libvirt close RPC.
func ConnectContext(ctx context.Context) (*libvirt.Libvirt, func() error, error) {
	conn, err := (&net.Dialer{}).DialContext(ctx, "unix", libvirtSocketPath)
	if err != nil {
		return nil, nil, fmt.Errorf("connecting to libvirt: %w", err)
	}
	return connectWithConn(ctx, conn)
}

func connectWithConn(ctx context.Context, conn net.Conn) (*libvirt.Libvirt, func() error, error) {
	closeConn := func() error {
		err := conn.Close()
		if errors.Is(err, net.ErrClosed) {
			return nil
		}
		return err
	}
	if err := ctx.Err(); err != nil {
		return nil, nil, errors.Join(err, closeConn())
	}
	stop := context.AfterFunc(ctx, func() { _ = closeConn() })
	lv := libvirt.NewWithDialer(dialers.NewAlreadyConnected(conn))
	err := lv.ConnectToURI(libvirt.QEMUSystem)
	stop()
	if err != nil || ctx.Err() != nil {
		return nil, nil, errors.Join(err, ctx.Err(), closeConn())
	}
	return lv, closeConn, nil
}
