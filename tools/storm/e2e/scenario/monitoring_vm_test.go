package scenario

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	"libvirt.org/go/libvirtxml"

	"tridenttools/pkg/virtdeploy"
)

func (o *serialTestOutput) Close() error { return nil }

func TestSerialMonitorSurvivesVMCleanup(t *testing.T) {
	if os.Getenv("TRIDENT_TEST_SERIAL_VM") != "1" {
		t.Skip("set TRIDENT_TEST_SERIAL_VM=1 to run against local qemu:///system")
	}
	for _, mode := range []string{"login", "cancel"} {
		t.Run(mode, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()
			lv, closeLibvirt, err := virtdeploy.ConnectContext(ctx)
			if err != nil {
				t.Fatal(err)
			}
			defer closeLibvirt()
			namespace := "storm-console-test-" + uuid.NewString()
			name := namespace + "-vm-0"
			logPath := filepath.Join("/tmp", name+"-serial0.log")
			t.Cleanup(func() {
				if err := virtdeploy.DeleteResources(namespace); err != nil {
					t.Error(err)
				}
				if output, err := exec.Command("sudo", "-n", "rm", "-f", "--", logPath).CombinedOutput(); err != nil {
					t.Errorf("remove own serial log: %v: %s", err, output)
				}
			})
			xml := fmt.Sprintf(`<domain type="kvm">
<name>%s</name><memory unit="MiB">128</memory><vcpu>1</vcpu>
<os><type arch="x86_64">hvm</type></os>
<devices>
<serial type="pty"><target port="0"/><log file="%s" append="off"/></serial>
<console type="pty"><target type="serial" port="0"/></console>
</devices>
</domain>`, name, logPath)
			domain, err := lv.DomainDefineXML(xml)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := lv.DomainCreateWithFlags(domain, uint32(libvirt.DomainStartPaused)); err != nil {
				t.Fatal(err)
			}
			definition := &libvirtxml.Domain{}
			if err := definition.Unmarshal(xml); err != nil {
				t.Fatal(err)
			}
			host := &testHostVirtDeploy{
				namespace: namespace,
				vm: virtdeploy.VirtDeployVMStatus{
					Name: name, Domain: domain, Definition: definition,
				},
				lv: lv, closeLibvirt: closeLibvirt,
			}
			s := TridentE2EScenario{hardware: HardwareTypeVM, testHost: host}
			output := &serialTestOutput{wrote: make(chan struct{})}
			done, err := s.spawnVMSerialMonitor(ctx, output)
			if err != nil {
				t.Fatal(err)
			}
			defer func() {
				if err := stopSerialMonitor(cancel, done); err != nil {
					t.Error(err)
				}
			}()
			text := "current boot marker\n"
			if mode == "login" {
				text += "azl login:"
			}
			write := exec.CommandContext(ctx, "sudo", "-n", "tee", "-a", "--", logPath)
			write.Stdin = strings.NewReader(text)
			if output, err := write.CombinedOutput(); err != nil {
				t.Fatalf("write own VM serial fixture: %v: %s", err, output)
			}
			select {
			case <-output.wrote:
			case <-ctx.Done():
				t.Fatal("privileged follower did not capture the VM log")
			}
			if mode == "login" {
				select {
				case <-done:
				case <-ctx.Done():
					t.Fatal("login prompt did not stop privileged follower")
				}
			}
			if err := host.Cleanup(); err != nil {
				t.Fatal(err)
			}
			if err := stopSerialMonitor(cancel, done); err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(output.String(), "current boot marker") {
				t.Fatalf("serial artifact did not survive teardown: %q", output.String())
			}
			if mode == "login" && strings.Contains(output.String(), "ERROR:") {
				t.Fatalf("login shutdown failed: %q", output.String())
			}
			t.Log("privileged follower reaped; VM deleted; serial artifact retained")
		})
	}
}
