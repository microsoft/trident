package proxies

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os/exec"
	"strings"
	"testing"
)

// requireDocker skips the test when Docker isn't available, so this test
// doesn't hard-fail on machines/CI runners without it. NebraskaProxy always
// needs a real ephemeral Postgres container - there is no in-memory
// alternative for github.com/flatcar/nebraska/backend.
func requireDocker(t *testing.T) {
	t.Helper()
	if _, err := exec.LookPath("docker"); err != nil {
		t.Skip("docker not available; skipping test that needs an ephemeral Postgres container")
	}
}

func postUpdateCheck(t *testing.T, addr, appID, machineID string) string {
	t.Helper()
	req := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0">
  <app appid="%s" version="1.0.0" track="west-us" machineid="%s">
    <updatecheck/>
  </app>
</request>`, appID, machineID)

	resp, err := http.Post(fmt.Sprintf("http://%s/", addr), "text/xml", strings.NewReader(req))
	if err != nil {
		t.Fatalf("POST updatecheck: %v", err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, body)
	}
	return string(body)
}

// TestNebraskaProxy exercises NebraskaProxy against the real
// github.com/flatcar/nebraska/backend Omaha handler and an ephemeral
// Postgres container (started/torn down per subtest), rather than a
// hand-rolled fake. Each subtest takes roughly 10-15s due to container
// startup and db migrations - slow for a unit test, but this is what
// validates the mock's seeding logic (app/package/channel/group, track
// matching, semver-driven grant/noupdate) independently of the full
// storm-trident VM suite, which additionally exercises the real
// trident-acl-agent binary end-to-end.
func TestNebraskaProxy(t *testing.T) {
	requireDocker(t)

	t.Run("update-available", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		t.Cleanup(cancel)

		p := &NebraskaProxy{Scenario: &NebraskaScenario{
			Available:   true,
			Version:     "5.0.0",
			URL:         "http://example.invalid/images/",
			SHA384:      "deadbeef",
			PackageName: "acl.cosi",
		}}
		listener, err := p.ListenAndServe(ctx, "127.0.0.1:0")
		if err != nil {
			t.Fatalf("ListenAndServe: %v", err)
		}

		if p.AppID() == "" {
			t.Fatal("expected non-empty AppID after seeding")
		}

		body := postUpdateCheck(t, listener.Addr().String(), p.AppID(), "smoke-test-machine")
		if !strings.Contains(body, `status="ok"`) {
			t.Fatalf("expected an ok updatecheck offering the package, got: %s", body)
		}
		if !strings.Contains(body, "5.0.0") {
			t.Fatalf("expected manifest version 5.0.0 in response, got: %s", body)
		}
	})

	t.Run("no-update", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		t.Cleanup(cancel)

		p := &NebraskaProxy{Scenario: &NebraskaScenario{Available: false}}
		listener, err := p.ListenAndServe(ctx, "127.0.0.1:0")
		if err != nil {
			t.Fatalf("ListenAndServe: %v", err)
		}

		body := postUpdateCheck(t, listener.Addr().String(), p.AppID(), "smoke-test-machine-2")
		if !strings.Contains(body, `status="noupdate"`) {
			t.Fatalf("expected noupdate status, got: %s", body)
		}
	})
}
