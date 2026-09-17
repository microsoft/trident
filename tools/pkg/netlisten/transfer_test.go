package netlisten

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"tridenttools/pkg/netlaunch"
)

// The host pulls multi-GB COSI images from the /files/ handler. A peer that
// disappears mid-transfer (connection reset on a long-haul link) must not hang
// or kill the listener: the run has to stay alive and diagnosable.
func TestPeerDisappearingMidTransferLeavesListenerServing(t *testing.T) {
	dir := t.TempDir()
	big := filepath.Join(dir, "regular.cosi")
	f, err := os.Create(big)
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if err := f.Truncate(64 << 20); err != nil { // 64 MiB is enough to abort mid-body
		t.Fatalf("truncate: %v", err)
	}
	f.Close()

	free, _ := net.Listen("tcp4", "0.0.0.0:0")
	port := free.Addr().(*net.TCPAddr).Port
	free.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	exit := make(chan error, 1)
	go func() {
		exit <- RunNetlisten(ctx, &netlaunch.NetListenConfig{
			NetCommonConfig: netlaunch.NetCommonConfig{
				ListenPort:     uint16(port),
				LogstreamFile:  filepath.Join(dir, "logstream.log"),
				ServeDirectory: dir,
			},
		})
	}()

	url := fmt.Sprintf("http://127.0.0.1:%d/files/regular.cosi", port)
	deadline := time.Now().Add(20 * time.Second)
	for {
		if resp, err := http.Get(fmt.Sprintf("http://127.0.0.1:%d/", port)); err == nil {
			resp.Body.Close()
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("listener never came up")
		}
		time.Sleep(50 * time.Millisecond)
	}

	// Begin the transfer, read a little, then abandon it mid-body.
	resp, err := http.Get(url)
	if err != nil {
		t.Fatalf("GET: %v", err)
	}
	if _, err := io.CopyN(io.Discard, resp.Body, 4096); err != nil {
		t.Fatalf("partial read: %v", err)
	}
	resp.Body.Close() // peer vanishes mid-body

	// The listener must still be alive and serving.
	select {
	case err := <-exit:
		t.Fatalf("listener exited after a mid-body abort: %v", err)
	case <-time.After(500 * time.Millisecond):
	}

	r2, err := http.Get(fmt.Sprintf("http://127.0.0.1:%d/", port))
	if err != nil {
		t.Fatalf("listener not serving after mid-body abort: %v", err)
	}
	r2.Body.Close()
}
