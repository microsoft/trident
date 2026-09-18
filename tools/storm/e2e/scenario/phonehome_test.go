package scenario

import (
	"context"
	"net"
	"path/filepath"
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

// The listener binds a fixed port, so every exit path must release it.
// Leaving one running makes the NEXT servicing case fail to bind with
// "address already in use", which reads as a product defect rather than a
// leaked goroutine.
func TestStopReleasesThePortForTheNextCase(t *testing.T) {
	dir := t.TempDir()
	free, err := net.Listen("tcp4", "0.0.0.0:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	port := free.Addr().(*net.TCPAddr).Port
	addr := free.Addr().String()
	free.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	start := func() (stopPhonehomeListener, error) {
		return startPhonehomeListener(ctx, &netlaunch.NetListenConfig{
			NetCommonConfig: netlaunch.NetCommonConfig{
				ListenPort:    uint16(port),
				LogstreamFile: filepath.Join(dir, "logstream.log"),
			},
		})
	}

	stop, err := start()
	if err != nil {
		t.Fatalf("first listener: %v", err)
	}
	stop()

	// A second listener on the same port must be able to bind, which is only
	// true if the first one genuinely released it.
	stop2, err := start()
	if err != nil {
		t.Fatalf("port was not released by stop(): %v", err)
	}
	stop2()

	// And once stopped, nothing is still serving on it.
	conn, err := net.DialTimeout("tcp", addr, 250*time.Millisecond)
	if err == nil {
		conn.Close()
		t.Error("something is still serving after stop()")
	}
}

// A readiness failure must not leave the goroutine holding the port either:
// the original startup error would then be masked by "address already in use"
// on every subsequent attempt.
func TestReadinessFailureDoesNotLeakThePort(t *testing.T) {
	occupied, err := net.Listen("tcp4", "0.0.0.0:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer occupied.Close()
	port := occupied.Addr().(*net.TCPAddr).Port

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	stop, err := startPhonehomeListener(ctx, &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:    uint16(port),
			LogstreamFile: filepath.Join(t.TempDir(), "logstream.log"),
		},
	})
	if err == nil {
		if stop != nil {
			stop()
		}
		t.Fatal("expected a startup failure when the port is already bound")
	}
	if stop != nil {
		t.Error("no stop function should be returned alongside an error")
	}
}
