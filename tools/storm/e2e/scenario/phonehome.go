package scenario

import (
	"context"
	"fmt"
	"net/http"
	"time"

	"github.com/sirupsen/logrus"

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
	// phonehomeStopTimeout bounds the wait for the listener to release the
	// port once cancelled.
	phonehomeStopTimeout = 30 * time.Second
)

// stopPhonehomeListener shuts the listener down and waits for it to release
// the port.
type stopPhonehomeListener func()

// startPhonehomeListener starts the phone-home/logstream/tracestream listener
// and returns once it is serving, together with a function that stops it.
//
// Waiting matters because callers trigger a servicing operation immediately
// afterwards: launching the listener in a bare goroutine lets the host phone
// home before the port is bound, and discards any bind error entirely. The
// failure then surfaces much later as a phone-home or reconnect timeout, which
// points at the host rather than at the listener that never started.
//
// Callers must defer the returned stop function. The listener binds a fixed
// port, so leaving one running past the end of its case makes the next
// servicing case fail to bind with "address already in use" -- a failure that
// would look like a product defect rather than a leaked goroutine. Every error
// path here shuts the listener down for the same reason: a listener that bound
// the port but never became probeable still holds it.
func startPhonehomeListener(ctx context.Context, config *netlaunch.NetListenConfig) (stopPhonehomeListener, error) {
	// Own the lifetime rather than borrowing the caller's context, so the
	// listener can be shut down independently of the case finishing.
	listenerCtx, cancel := context.WithCancel(ctx)
	exit := make(chan error, 1)
	go func() { exit <- netlisten.RunNetlisten(listenerCtx, config) }()

	// waitForExit bounds how long we block on the goroutine: shutdown is
	// prompt, and hanging here would be worse than the leak we are preventing.
	waitForExit := func() {
		select {
		case <-exit:
		case <-time.After(phonehomeStopTimeout):
			logrus.Warnf("phone-home listener did not exit within %s", phonehomeStopTimeout)
		}
	}
	shutdown := func() {
		cancel()
		waitForExit()
	}

	// A bind failure is immediate, so give it a moment to surface before
	// probing the port. Without this the probe can succeed against whatever
	// process already holds the port and report a listener that never started.
	select {
	case err := <-exit:
		cancel()
		if err != nil {
			return nil, fmt.Errorf("phone-home listener failed to start: %w", err)
		}
		return nil, fmt.Errorf("phone-home listener exited before serving requests")
	case <-time.After(phonehomeBindGrace):
	case <-ctx.Done():
		shutdown()
		return nil, ctx.Err()
	}

	address := fmt.Sprintf("127.0.0.1:%d", config.ListenPort)
	// Probe with a real HTTP round-trip rather than a bare TCP dial. The port
	// is bound early in RunNetlisten, but the logstream and tracestream
	// handlers are set up afterwards and server.Serve starts later again -- so
	// a dial can succeed against a listener that is about to error out. Only a
	// completed response proves Serve is running with its handlers installed.
	// Any status counts, including 404.
	probeURL := fmt.Sprintf("http://%s/", address)
	client := &http.Client{Timeout: time.Second}
	deadline := time.Now().Add(phonehomeReadyTimeout)

	for {
		select {
		case err := <-exit:
			cancel()
			if err != nil {
				return nil, fmt.Errorf("phone-home listener failed to start: %w", err)
			}
			return nil, fmt.Errorf("phone-home listener exited before serving requests")
		default:
		}

		resp, err := client.Get(probeURL)
		if err == nil {
			resp.Body.Close()
			return shutdown, nil
		}

		if time.Now().After(deadline) {
			shutdown()
			return nil, fmt.Errorf("phone-home listener was not serving on %s after %s: %w",
				address, phonehomeReadyTimeout, err)
		}

		select {
		case <-ctx.Done():
			shutdown()
			return nil, ctx.Err()
		case <-time.After(phonehomeReadyPoll):
		}
	}
}
