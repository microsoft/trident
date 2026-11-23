package netlisten

import (
	"context"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/phonehome"
)

// A setup failure after a successful bind must not leave the port occupied:
// the next scenario or retry in the same runner would then fail with "address
// already in use" instead of the real error.
func TestRunNetlistenReleasesPortOnSetupFailure(t *testing.T) {
	free, err := net.Listen("tcp4", "0.0.0.0:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	port := free.Addr().(*net.TCPAddr).Port
	free.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	dir := filepath.Join("test-output", "netlisten-"+strconv.Itoa(os.Getpid()))
	if err := os.RemoveAll(dir); err != nil {
		t.Fatalf("remove old test output: %v", err)
	}
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatalf("create test output: %v", err)
	}
	defer os.RemoveAll(dir)

	err = RunNetlisten(ctx, &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:    uint16(port),
			LogstreamFile: filepath.Join(dir, "no-such-dir", "logstream.log"),
		},
	})
	if err == nil {
		t.Fatal("expected a setup failure for an uncreatable logstream path")
	}

	// The port must be reusable immediately.
	again, err := net.Listen("tcp4", free.Addr().String())
	if err != nil {
		t.Fatalf("port still occupied after setup failure: %v", err)
	}
	again.Close()
}

// A serve failure must cut the phone-home wait short. Capturing the error but
// still blocking would leave the run to time out against its full deadline and
// report the host as unresponsive, hiding the real cause.
func TestServeAndWaitReturnsWhenServeFails(t *testing.T) {
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to listen: %v", err)
	}
	// Kill the listener so Serve fails immediately on its first Accept.
	listener.Close()

	// Far longer than this test should take: if the wait is not cancelled, the
	// deadline below is what would end it, and the test fails on elapsed time.
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// Never written to, standing in for a host that never phones home.
	result := make(chan phonehome.PhoneHomeResult, 1)

	start := time.Now()
	err = serveAndWait(ctx, &http.Server{Handler: http.NewServeMux()}, listener, result, 0)
	elapsed := time.Since(start)

	if err == nil {
		t.Fatal("expected an error when the server cannot serve, got nil")
	}
	if !strings.Contains(err.Error(), "phone-home server failed") {
		t.Fatalf("expected the serve failure to be reported, got: %v", err)
	}
	if elapsed > 10*time.Second {
		t.Fatalf("serve failure took %v to surface; the wait was not cancelled", elapsed)
	}
}
