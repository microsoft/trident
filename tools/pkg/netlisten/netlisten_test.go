package netlisten

import (
	"context"
	"net"
	"path/filepath"
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

	// An uncreatable tracestream path fails after the bind has already
	// happened. (SetupLogstream calls logrus.Fatal instead of returning, so it
	// cannot be used to exercise this path from a test.)
	dir := t.TempDir()
	err = RunNetlisten(ctx, &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort: uint16(port),
			// A valid logstream path is required: SetupLogstream calls
			// logrus.Fatal rather than returning an error, so it would take the
			// test process down before the path under test is reached.
			LogstreamFile:   filepath.Join(dir, "logstream.log"),
			TracestreamFile: filepath.Join(dir, "no-such-dir", "trace.log"),
		},
	})
	if err == nil {
		t.Fatal("expected a setup failure for an uncreatable tracestream path")
	}

	// The port must be reusable immediately.
	again, err := net.Listen("tcp4", free.Addr().String())
	if err != nil {
		t.Fatalf("port still occupied after setup failure: %v", err)
	}
	again.Close()
}
