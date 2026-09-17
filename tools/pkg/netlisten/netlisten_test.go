package netlisten

import (
	"context"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"testing"
	"time"

	"tridenttools/pkg/netlaunch"
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
