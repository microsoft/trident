package sshutils

import (
	"bytes"
	"context"
	"errors"
	"os"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

func TestFacilitiesGuest(t *testing.T) {
	host := os.Getenv("TRIDENT_FACILITIES_GUEST")
	if host == "" {
		t.Skip("set TRIDENT_FACILITIES_GUEST and TRIDENT_FACILITIES_KEY for a disposable guest")
	}
	key, err := os.ReadFile(os.Getenv("TRIDENT_FACILITIES_KEY"))
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	cfg := SshClientConfig{Host: host, User: "testing-user", PrivateKey: key}
	client, err := CreateSshClient(ctx, cfg)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()

	path, err := CommandOutput(client, "sudo mktemp /root/trident-facilities-XXXXXX")
	if err != nil {
		t.Fatal(err)
	}
	path = strings.TrimSpace(path)
	defer func() {
		cleanupCtx, stop := context.WithTimeout(context.Background(), 10*time.Second)
		defer stop()
		cleanupClient, err := CreateSshClient(cleanupCtx, cfg)
		if err != nil {
			t.Error(err)
			return
		}
		defer cleanupClient.Close()
		if _, err := CommandOutput(cleanupClient, "sudo rm -f -- "+ShellQuote(path)); err != nil {
			t.Error(err)
		}
	}()
	const contents = "host-configuration: facilities\n"
	if err := client.Upload(ctx, path, strings.NewReader(contents)); err != nil {
		t.Fatal(err)
	}
	var fetched bytes.Buffer
	if err := client.Fetch(ctx, path, &fetched); err != nil {
		t.Fatal(err)
	}
	if fetched.String() != contents {
		t.Fatalf("privileged transfer changed contents: %q", fetched.String())
	}
	mode, err := CommandOutput(client, "sudo stat -c '%a:%u' -- "+ShellQuote(path))
	if err != nil || strings.TrimSpace(mode) != "600:0" {
		t.Fatalf("uploaded file is not root-owned and private: %q, %v", mode, err)
	}

	var journal bytes.Buffer
	result, err := client.RunIO(ctx, "printf stdout; printf stderr >&2", nil, &journal, &journal)
	if err != nil || result.ExitCode != 0 || !strings.Contains(journal.String(), "stdout") || !strings.Contains(journal.String(), "stderr") {
		t.Fatalf("streamed command: %+v, %q, %v", result, journal.String(), err)
	}
	out, err := RunCommand(client, "exit 7")
	if err != nil || out.Status != 7 || out.Check() == nil {
		t.Fatalf("remote exit status changed: %+v, %v", out, err)
	}
	waitCtx, stop := context.WithTimeout(ctx, 10*time.Millisecond)
	defer stop()
	if err := WaitForDisconnect(waitCtx, client); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("live connection was reported disconnected: %v", err)
	}
	if os.Getenv("TRIDENT_FACILITIES_REBOOT") == "1" {
		out, err := RunCommand(client, "sudo reboot")
		var missing *ssh.ExitMissingError
		if err != nil && !errors.As(err, &missing) {
			t.Fatalf("reboot error lost its SSH cause: %v", err)
		}
		if err == nil && out.Status != 0 {
			t.Fatalf("reboot failed: %s", out.Report())
		}
	} else {
		client.Close()
	}
	if err := WaitForDisconnect(ctx, client); err != nil {
		t.Fatal(err)
	}
	client, err = CreateSshClientWithRedial(ctx, time.Millisecond, cfg)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	if _, err := CommandOutput(client, "true"); err != nil {
		t.Fatal(err)
	}
}
