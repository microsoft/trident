package scenario

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

type serialTestOutput struct {
	bytes.Buffer
	wrote chan struct{}
	once  sync.Once
}

func (o *serialTestOutput) Write(data []byte) (int, error) {
	n, err := o.Buffer.Write(data)
	o.once.Do(func() { close(o.wrote) })
	return n, err
}

func nativeSerialCommand(t *testing.T, ctx context.Context, path string) *exec.Cmd {
	t.Helper()
	cmd, err := serialLogCommand(ctx, path)
	if err != nil {
		t.Fatal(err)
	}
	// Unit tests exercise the same tail arguments without needing sudo.
	native := exec.CommandContext(ctx, cmd.Args[3], cmd.Args[4:]...)
	native.Cancel = cmd.Cancel
	return native
}

func TestSerialMonitorFollowsCurrentBoot(t *testing.T) {
	for _, mode := range []string{"append", "truncate", "replace", "create", "truncate-before-start"} {
		t.Run(mode, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "serial.log")
			if mode != "create" {
				if err := os.WriteFile(path, []byte(strings.Repeat("old boot\n", 1000)+"old-host login:\n"), 0600); err != nil {
					t.Fatal(err)
				}
			}
			ctx, cancel := context.WithCancel(context.Background())
			output := &serialTestOutput{wrote: make(chan struct{})}
			cmd := nativeSerialCommand(t, ctx, path)
			if mode == "truncate-before-start" {
				if err := os.WriteFile(path, nil, 0600); err != nil {
					t.Fatal(err)
				}
			}
			result := make(chan error, 1)
			started := make(chan error, 1)
			finished := make(chan struct{})
			go func() {
				result <- runSerialMonitor(ctx, cancel, cmd, output, started)
				close(finished)
			}()
			t.Cleanup(func() {
				cancel()
				select {
				case <-finished:
				case <-time.After(3 * time.Second):
					t.Error("serial follower survived cancellation")
				}
			})
			select {
			case err := <-started:
				if err != nil {
					t.Fatal(err)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("serial follower did not attach")
			}

			file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := file.WriteString("live boot marker\n"); err != nil {
				t.Fatal(err)
			}
			file.Close()
			select {
			case <-output.wrote:
			case err := <-result:
				t.Fatalf("old login prompt satisfied new monitor: %v", err)
			case <-time.After(3 * time.Second):
				t.Fatal("serial follower did not read new output")
			}

			switch mode {
			case "truncate":
				err = os.WriteFile(path, []byte("new boot\nazl login:"), 0600)
			case "replace":
				replacement := path + ".new"
				if err = os.WriteFile(replacement, []byte("new boot\nazl login:"), 0600); err == nil {
					err = os.Rename(replacement, path)
				}
			default:
				file, err = os.OpenFile(path, os.O_WRONLY|os.O_APPEND, 0600)
				if err == nil {
					_, err = file.WriteString("azl login:")
					file.Close()
				}
			}
			if err != nil {
				t.Fatal(err)
			}
			select {
			case err := <-result:
				if err != nil {
					t.Fatal(err)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("login prompt did not stop serial follower")
			}
			if strings.Contains(output.String(), "old boot") || !strings.Contains(output.String(), "live boot marker") {
				t.Fatalf("incorrect current-boot artifact: %q", output.String())
			}
			if cmd.ProcessState == nil {
				t.Fatal("serial follower process was not reaped")
			}
		})
	}
}

func TestSerialMonitorCancelsIdleFollower(t *testing.T) {
	path := filepath.Join(t.TempDir(), "serial.log")
	if err := os.WriteFile(path, nil, 0600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cmd := nativeSerialCommand(t, ctx, path)
	output := &serialTestOutput{wrote: make(chan struct{})}
	done := make(chan struct{})
	result := make(chan error, 1)
	started := make(chan error, 1)
	go func() {
		result <- runSerialMonitor(ctx, cancel, cmd, output, started)
		close(done)
	}()
	select {
	case err := <-started:
		if err != nil {
			cancel()
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		cancel()
		t.Fatal("idle follower did not attach")
	}
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND, 0600)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.WriteString("boot marker\n"); err != nil {
		t.Fatal(err)
	}
	file.Close()
	select {
	case <-output.wrote:
	case <-time.After(3 * time.Second):
		cancel()
		t.Fatal("idle follower never started")
	}
	if err := stopSerialMonitor(cancel, done); err != nil {
		t.Fatal(err)
	}
	if err := <-result; !errors.Is(err, context.Canceled) {
		t.Fatalf("cancellation was lost: %v", err)
	}
}

func TestSerialMonitorReportsFollowerFailure(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestSerialMonitorChild$")
	cmd.Env = append(os.Environ(), "TRIDENT_SERIAL_MONITOR_CHILD=failure")
	err := runSerialMonitor(ctx, cancel, cmd, io.Discard, make(chan error, 1))
	if err == nil || !strings.Contains(err.Error(), "serial source unavailable") {
		t.Fatalf("follower failure was lost: %v", err)
	}
}

type failingSerialOutput struct {
	err error
}

func (o failingSerialOutput) Write([]byte) (int, error) { return 0, o.err }

func TestSerialMonitorReportsArtifactFailure(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestSerialMonitorChild$")
	cmd.Env = append(os.Environ(), "TRIDENT_SERIAL_MONITOR_CHILD=1")
	want := errors.New("artifact writer failed")
	err := runSerialMonitor(ctx, cancel, cmd, failingSerialOutput{err: want}, make(chan error, 1))
	if !errors.Is(err, want) {
		t.Fatalf("artifact failure was lost: %v", err)
	}
}

func TestSerialMonitorChild(t *testing.T) {
	switch os.Getenv("TRIDENT_SERIAL_MONITOR_CHILD") {
	case "failure":
		fmt.Fprintln(os.Stderr, "serial source unavailable")
		os.Exit(2)
	case "1":
	default:
		return
	}
	fmt.Print("==> serial.log <==\nboot output\n")
	time.Sleep(time.Hour)
}
