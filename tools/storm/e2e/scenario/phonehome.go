package scenario

import (
	"context"
	"fmt"
	"net"
	"time"

	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/netlisten"
)

const (
	// phonehomeReadyTimeout bounds how long to wait for the listener to accept
	// connections. Binding a port is immediate; this only has to cover
	// goroutine scheduling.
	phonehomeReadyTimeout = 30 * time.Second
	phonehomeReadyPoll    = 50 * time.Millisecond
	// phonehomeBindGrace is how long to wait for an immediate bind failure
	// before probing the port.
	phonehomeBindGrace = 250 * time.Millisecond
)

// startPhonehomeListener starts the phone-home/logstream/tracestream listener
// and returns once it is accepting connections.
//
// Waiting matters because callers trigger a servicing operation immediately
// afterwards: launching the listener in a bare goroutine lets the host phone
// home before the port is bound, and discards any bind error entirely. The
// failure then surfaces much later as a phone-home or reconnect timeout, which
// points at the host rather than at the listener that never started.
//
// The returned channel carries the listener's eventual exit error, so callers
// that care can observe a late failure too.
func startPhonehomeListener(ctx context.Context, config *netlaunch.NetListenConfig) (<-chan error, error) {
	exit := make(chan error, 1)
	go func() { exit <- netlisten.RunNetlisten(ctx, config) }()

	// A bind failure is immediate, so give it a moment to surface before
	// probing the port. Without this the probe can succeed against whatever
	// process already holds the port and report a listener that never started.
	select {
	case err := <-exit:
		if err != nil {
			return nil, fmt.Errorf("phone-home listener failed to start: %w", err)
		}
		return nil, fmt.Errorf("phone-home listener exited before accepting connections")
	case <-time.After(phonehomeBindGrace):
	case <-ctx.Done():
		return nil, ctx.Err()
	}

	address := fmt.Sprintf("127.0.0.1:%d", config.ListenPort)
	deadline := time.Now().Add(phonehomeReadyTimeout)

	for {
		// An exit before the port answers is always a startup failure: this
		// listener is meant to stay up until the servicing operation reports.
		select {
		case err := <-exit:
			if err != nil {
				return nil, fmt.Errorf("phone-home listener failed to start: %w", err)
			}
			return nil, fmt.Errorf("phone-home listener exited before accepting connections")
		default:
		}

		conn, err := net.DialTimeout("tcp", address, time.Second)
		if err == nil {
			conn.Close()
			return exit, nil
		}

		if time.Now().After(deadline) {
			return nil, fmt.Errorf("phone-home listener was not accepting connections on %s after %s: %w",
				address, phonehomeReadyTimeout, err)
		}

		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(phonehomeReadyPoll):
		}
	}
}
