package scenario

import (
	"context"
	"net"
	"testing"
	"time"

	"tridenttools/pkg/netlaunch"
)

// A bind failure used to be discarded: the listener was launched in a bare
// goroutine, so the rollback proceeded and failed later as a phone-home
// timeout, pointing at the host instead of the listener that never started.
func TestStartPhonehomeListenerReportsBindFailure(t *testing.T) {
	// Occupy the port so the listener cannot bind.
	occupied, err := net.Listen("tcp4", "0.0.0.0:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer occupied.Close()
	port := occupied.Addr().(*net.TCPAddr).Port

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	_, err = startPhonehomeListener(ctx, &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{ListenPort: uint16(port)},
	})
	if err == nil {
		t.Fatal("expected a startup error when the port is already bound, got nil")
	}
}

// The happy path must still return promptly once the port is accepting, or
// every servicing scenario would stall by the full readiness timeout.
func TestStartPhonehomeListenerBecomesReady(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	free, err := net.Listen("tcp4", "0.0.0.0:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	port := free.Addr().(*net.TCPAddr).Port
	free.Close() // release it for the listener under test

	start := time.Now()
	exit, err := startPhonehomeListener(ctx, &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:    uint16(port),
			LogstreamFile: t.TempDir() + "/logstream.log",
		},
	})
	if err != nil {
		t.Fatalf("startPhonehomeListener: %v", err)
	}
	if exit == nil {
		t.Fatal("expected an exit channel on success")
	}
	if elapsed := time.Since(start); elapsed > phonehomeReadyTimeout {
		t.Errorf("took %s to report ready", elapsed)
	}

	// The port must genuinely be accepting by the time it returns.
	conn, err := net.DialTimeout("tcp", free.Addr().String(), time.Second)
	if err != nil {
		t.Fatalf("listener not accepting after ready: %v", err)
	}
	conn.Close()
}
