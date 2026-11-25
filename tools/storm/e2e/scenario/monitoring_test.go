package scenario

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/digitalocean/go-libvirt/socket/dialers"
	"github.com/google/uuid"
)

func TestSerialMonitorLifecycle(t *testing.T) {
	for _, mode := range []string{"login", "cancel", "failure", "retry", "eof"} {
		t.Run(mode, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			defer cancel()
			var output bytes.Buffer
			var lastCommand *exec.Cmd
			attempts := 0
			err := runSerialMonitor(ctx, cancel, func() *exec.Cmd {
				attempts++
				childMode := mode
				if mode == "retry" {
					childMode = "inactive"
					if attempts > 1 {
						childMode = "login"
					}
				}
				cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestSerialConsoleChild$")
				cmd.Env = append(os.Environ(), "TRIDENT_SERIAL_CONSOLE_CHILD="+childMode)
				lastCommand = cmd
				return cmd
			}, &output)
			switch mode {
			case "login", "retry":
				if err != nil {
					t.Fatal(err)
				}
				if !strings.Contains(output.String(), "current boot") || !strings.Contains(output.String(), "mos login:") {
					t.Fatalf("serial output lost or installer prompt accepted: %q", output.String())
				}
				if mode == "retry" && attempts != 2 {
					t.Fatalf("expected one retry for inactive domain, got %d attempts", attempts)
				}
			case "cancel":
				if !errors.Is(err, context.DeadlineExceeded) {
					t.Fatalf("cancellation lost: %v", err)
				}
			case "failure":
				if err == nil || !strings.Contains(err.Error(), "console denied") {
					t.Fatalf("console failure lost: %v", err)
				}
			case "eof":
				if err == nil || !strings.Contains(err.Error(), "before the login prompt") {
					t.Fatalf("unexpected console EOF lost: %v", err)
				}
			}
			if lastCommand.ProcessState == nil {
				t.Fatal("console process was not reaped")
			}
		})
	}
}

func TestSerialConsoleChild(t *testing.T) {
	switch os.Getenv("TRIDENT_SERIAL_CONSOLE_CHILD") {
	case "login":
		fmt.Print("mos login:\ncurrent boot\nazl login:")
		for {
			fmt.Print("remaining console output\n")
		}
	case "cancel":
		fmt.Print("current boot\n")
		time.Sleep(time.Minute)
	case "failure":
		fmt.Fprintln(os.Stderr, "console denied")
		os.Exit(1)
	case "inactive":
		fmt.Fprintln(os.Stderr, "domain is not running")
		os.Exit(1)
	case "eof":
		os.Exit(0)
	}
}

type failingSerialOutput struct{ err error }

func (o failingSerialOutput) Write([]byte) (int, error) { return 0, o.err }

func TestSerialMonitorArtifactFailure(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	want := errors.New("artifact writer failed")
	var child *exec.Cmd
	err := runSerialMonitor(ctx, cancel, func() *exec.Cmd {
		child = exec.CommandContext(ctx, os.Args[0], "-test.run=^TestSerialConsoleChild$")
		child.Env = append(os.Environ(), "TRIDENT_SERIAL_CONSOLE_CHILD=login")
		return child
	}, failingSerialOutput{want})
	if !errors.Is(err, want) {
		t.Fatalf("artifact failure lost: %v", err)
	}
	if child.ProcessState == nil {
		t.Fatal("console process survived artifact failure")
	}
}

func TestSerialMonitorSkipsCanceledStart(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	err := runSerialMonitor(ctx, cancel, func() *exec.Cmd {
		return exec.CommandContext(ctx, "unused-console")
	}, io.Discard)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("cancellation lost: %v", err)
	}
}

func TestSerialConsoleRootlessVM(t *testing.T) {
	if os.Getenv("TRIDENT_TEST_SERIAL_VM") != "1" {
		t.Skip("set TRIDENT_TEST_SERIAL_VM=1 to test local unprivileged libvirt access")
	}
	if os.Geteuid() == 0 {
		t.Fatal("this regression must run without root")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	// This control connection must remain usable after the monitor exits.
	lv := libvirt.NewWithDialer(dialers.NewLocal())
	if err := lv.Connect(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := lv.Disconnect(); err != nil {
			t.Error(err)
		}
	})
	name := "storm-rootless-console-" + uuid.NewString()
	xml := fmt.Sprintf(`<domain type="kvm"><name>%s</name>
<memory unit="MiB">128</memory><vcpu>1</vcpu><os><type arch="x86_64">hvm</type></os>
<devices><serial type="pty"><target port="0"/></serial>
<console type="pty"><target type="serial" port="0"/></console></devices></domain>`, name)
	domain, err := lv.DomainDefineXML(xml)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := lv.DomainDestroy(domain); err != nil {
			t.Error(err)
		}
		if err := lv.DomainUndefine(domain); err != nil {
			t.Error(err)
		}
	})
	if _, err := lv.DomainCreateWithFlags(domain, uint32(libvirt.DomainStartPaused)); err != nil {
		t.Fatal(err)
	}
	var output bytes.Buffer
	childCtx, stop := context.WithTimeout(ctx, 200*time.Millisecond)
	defer stop()
	err = monitorVMConsole(childCtx, stop, name, &output)
	if !errors.Is(err, context.DeadlineExceeded) || !strings.Contains(output.String(), "Connected to domain") {
		t.Fatalf("rootless console did not connect and cancel: %v: %s", err, output.String())
	}
	if _, _, err := lv.DomainGetState(domain, 0); err != nil {
		t.Fatalf("shared libvirt connection unusable after monitor cancellation: %v", err)
	}
	t.Log("non-root console connected and canceled; VM remained accessible for teardown")
}
