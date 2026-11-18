package qemu

import (
	"fmt"
	"net/url"
	"os"
	"runtime"
	"testing"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
)

// Opt-in: needs libvirt access, but defines only inactive, diskless test domains.
func TestLibvirtConnectionLifetime(t *testing.T) {
	if os.Getenv("TRIDENT_TEST_LIBVIRT") != "1" {
		t.Skip("set TRIDENT_TEST_LIBVIRT=1 to exercise real libvirt connection cleanup")
	}
	uri, err := url.Parse(string(libvirt.QEMUSession))
	if err != nil {
		t.Fatal(err)
	}
	lv, err := libvirt.ConnectToURI(uri)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = lv.Disconnect() })
	before := runtime.NumGoroutine()
	cfg := QemuConfig{}
	for i := 0; i < 4; i++ {
		name := "trident-connection-test-" + uuid.NewString()
		domain, err := lv.DomainDefineXML(fmt.Sprintf(`<domain type='qemu'><name>%s</name><memory unit='MiB'>128</memory><vcpu>1</vcpu><os><type arch='x86_64'>hvm</type></os></domain>`, name))
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() {
			_, _, err := lv.DomainGetState(domain, 0)
			if err == nil {
				_ = lv.DomainUndefineFlags(domain, 0)
			}
		})
		// Both lookup-success and missing-domain paths must release their connections.
		_, _ = cfg.GetVmIPAddresses(name)
		if err := cfg.CleanupQemuVM(name); err != nil {
			t.Fatal(err)
		}
		if _, err := cfg.GetVmIPAddresses(name); err == nil {
			t.Fatal("expected missing-domain error")
		}
		if err := cfg.CleanupQemuVM(name); err != nil {
			t.Fatal(err)
		}
	}
	deadline := time.Now().Add(3 * time.Second)
	for runtime.NumGoroutine() > before && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	after := runtime.NumGoroutine()
	t.Logf("libvirt connections: goroutines before=%d after=%d", before, after)
	if after > before {
		stack := make([]byte, 32*1024)
		stack = stack[:runtime.Stack(stack, true)]
		t.Fatalf("libvirt helper paths leaked %d goroutines:\n%s", after-before, stack)
	}
}
