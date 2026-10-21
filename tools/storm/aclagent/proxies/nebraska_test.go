package proxies

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net/http"
	"os/exec"
	"strings"
	"testing"

	"github.com/flatcar/nebraska/backend/pkg/api"
)

// newTestHTTPSClient returns an http.Client that trusts exactly certPEM (and
// nothing else), mirroring how the real NebraskaProxy is only ever trusted
// via an explicit CA cert path/system-trust-store install, never via
// disabled verification.
func newTestHTTPSClient(t *testing.T, certPEM []byte) *http.Client {
	t.Helper()
	pool := x509.NewCertPool()
	if !pool.AppendCertsFromPEM(certPEM) {
		t.Fatalf("failed to parse test CA cert PEM")
	}
	return &http.Client{
		Transport: &http.Transport{
			TLSClientConfig: &tls.Config{RootCAs: pool},
		},
	}
}

// newTestNebraskaProxy generates a fresh ephemeral TLS cert for listenAddr's
// host and returns a NebraskaProxy configured to serve it, plus an
// http.Client that trusts it, so subtests never fall back to disabling TLS
// verification.
func newTestNebraskaProxy(t *testing.T, scenario *NebraskaScenario) (*NebraskaProxy, *http.Client) {
	t.Helper()
	cert, certPEM, err := GenerateEphemeralTLSCert("127.0.0.1")
	if err != nil {
		t.Fatalf("GenerateEphemeralTLSCert: %v", err)
	}
	return &NebraskaProxy{Scenario: scenario, Cert: cert}, newTestHTTPSClient(t, certPEM)
}

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

func postUpdateCheck(t *testing.T, client *http.Client, addr, appID, machineID string) string {
	t.Helper()
	req := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0">
  <app appid="%s" version="1.0.0" track="west-us" machineid="%s">
    <updatecheck/>
  </app>
</request>`, appID, machineID)

	resp, err := client.Post(fmt.Sprintf("https://%s/", addr), "text/xml", strings.NewReader(req))
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

// postEvent posts a bare progress/terminal event, mirroring the (eventtype,
// eventresult) requests trident-acl-agent's nebraska::Client sends during
// stage/finalize/post-reboot-commit (see
// crates/trident-acl-agent/src/nebraska/event.rs for the whitelisted pairs).
func postEvent(t *testing.T, client *http.Client, addr, appID, machineID string, eventType, eventResult int) string {
	t.Helper()
	req := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0">
  <app appid="%s" version="1.0.0" track="west-us" machineid="%s">
    <event eventtype="%d" eventresult="%d"></event>
  </app>
</request>`, appID, machineID, eventType, eventResult)

	resp, err := client.Post(fmt.Sprintf("https://%s/", addr), "text/xml", strings.NewReader(req))
	if err != nil {
		t.Fatalf("POST event(%d,%d): %v", eventType, eventResult, err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, body)
	}
	return string(body)
}

// postUpdateCheckWithCookie is like postUpdateCheck but lets the caller set
// an explicit Cookie header (empty means none), and returns the response
// headers rather than the body - for asserting Set-Cookie/affinity-cookie
// behavior without needing a cookie-jar-enabled client (trident-acl-agent's
// own Client decides per request, not via a jar, whether it has a cookie to
// send yet - see crates/trident-acl-agent/src/core/nebraska/client.rs).
func postUpdateCheckWithCookie(t *testing.T, client *http.Client, addr, appID, machineID, cookie string) http.Header {
	t.Helper()
	body := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0">
  <app appid="%s" version="1.0.0" track="west-us" machineid="%s">
    <updatecheck/>
  </app>
</request>`, appID, machineID)

	req, err := http.NewRequest(http.MethodPost, fmt.Sprintf("https://%s/", addr), strings.NewReader(body))
	if err != nil {
		t.Fatalf("building request: %v", err)
	}
	req.Header.Set("Content-Type", "text/xml")
	if cookie != "" {
		req.Header.Set("Cookie", cookie)
	}
	resp, err := client.Do(req)
	if err != nil {
		t.Fatalf("POST updatecheck: %v", err)
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, respBody)
	}
	return resp.Header
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

		p, client := newTestNebraskaProxy(t, &NebraskaScenario{
			Available:   true,
			Version:     "5.0.0",
			URL:         "http://example.invalid/images/",
			SHA384:      strings.Repeat("ab", 48),
			PackageName: "acl.cosi",
		})
		listener, stop, err := p.ListenAndServe(ctx, "127.0.0.1:0")
		if err != nil {
			t.Fatalf("ListenAndServe: %v", err)
		}
		t.Cleanup(func() { _ = stop() })

		if p.AppID() == "" {
			t.Fatal("expected non-empty AppID after seeding")
		}

		body := postUpdateCheck(t, client, listener.Addr().String(), p.AppID(), "smoke-test-machine")
		if !strings.Contains(body, `status="ok"`) {
			t.Fatalf("expected an ok updatecheck offering the package, got: %s", body)
		}
		if !strings.Contains(body, "5.0.0") {
			t.Fatalf("expected manifest version 5.0.0 in response, got: %s", body)
		}
		assertCosiMetadataLabel(t, body, "acl.cosi", testMetadataSHA384)
	})

	t.Run("invalid-metadata-sha384", func(t *testing.T) {
		for _, digest := range []string{"not-hex", strings.Repeat("ab", 47), strings.Repeat("ab", 49)} {
			t.Run(fmt.Sprintf("length-%d", len(digest)), func(t *testing.T) {
				ctx, cancel := context.WithCancel(context.Background())
				defer cancel()
				p, _ := newTestNebraskaProxy(t, &NebraskaScenario{
					Available: true,
					Version:   "5.0.0",
					SHA384:    digest,
				})
				_, stop, err := p.ListenAndServe(ctx, "127.0.0.1:0")
				if err == nil {
					_ = stop()
					t.Fatal("expected invalid metadata SHA384 to fail seeding")
				}
				if !strings.Contains(err.Error(), "SHA384") {
					t.Fatalf("expected SHA384 validation error, got: %v", err)
				}
			})
		}
	})

	t.Run("no-update", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		t.Cleanup(cancel)

		p, client := newTestNebraskaProxy(t, &NebraskaScenario{Available: false})
		listener, stop, err := p.ListenAndServe(ctx, "127.0.0.1:0")
		if err != nil {
			t.Fatalf("ListenAndServe: %v", err)
		}
		t.Cleanup(func() { _ = stop() })

		body := postUpdateCheck(t, client, listener.Addr().String(), p.AppID(), "smoke-test-machine-2")
		if !strings.Contains(body, `status="noupdate"`) {
			t.Fatalf("expected noupdate status, got: %s", body)
		}
	})

	t.Run("status-history-matches-full-update-sequence", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		t.Cleanup(cancel)

		p, client := newTestNebraskaProxy(t, &NebraskaScenario{
			Available:   true,
			Version:     "5.0.0",
			URL:         "http://example.invalid/images/",
			SHA384:      strings.Repeat("ab", 48),
			PackageName: "acl.cosi",
		})
		listener, stop, err := p.ListenAndServe(ctx, "127.0.0.1:0")
		if err != nil {
			t.Fatalf("ListenAndServe: %v", err)
		}
		t.Cleanup(func() { _ = stop() })
		addr := listener.Addr().String()
		machineID := "status-history-machine"

		// Drive the instance through exactly the same request sequence a
		// real trident-acl-agent run-ab-update does: an updatecheck grants
		// the update, then stage/finalize/post-reboot-commit each report one
		// progress or terminal event (see event.rs's whitelisted pairs).
		postUpdateCheck(t, client, addr, p.AppID(), machineID)
		postEvent(t, client, addr, p.AppID(), machineID, 13, 1)  // DownloadStarted
		postEvent(t, client, addr, p.AppID(), machineID, 14, 1)  // DownloadFinished
		postEvent(t, client, addr, p.AppID(), machineID, 800, 1) // Installed
		postEvent(t, client, addr, p.AppID(), machineID, 3, 2)   // Completed (success+reboot)

		if err := p.ValidateStatusHistory(ExpectedUpdateStatusSequence); err != nil {
			t.Fatalf("ValidateStatusHistory: %v", err)
		}

		// A truncated/reordered history must be rejected, not silently
		// accepted - otherwise ValidateStatusHistory would be a no-op check.
		if err := p.ValidateStatusHistory([]int{api.InstanceStatusUpdateGranted}); err == nil {
			t.Fatal("expected ValidateStatusHistory to reject a truncated status sequence")
		}
	})

	// This simulated affinity cookie exists purely for storm's own
	// assertions (see RunABUpdate in tools/storm/aclagent/tests/update.go) -
	// real Nebraska's own Omaha handler, exercised by every other subtest
	// above, never sets one itself.
	t.Run("affinity-cookie-issued-and-echoed", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		t.Cleanup(cancel)

		p, client := newTestNebraskaProxy(t, &NebraskaScenario{Available: false})
		listener, stop, err := p.ListenAndServe(ctx, "127.0.0.1:0")
		if err != nil {
			t.Fatalf("ListenAndServe: %v", err)
		}
		t.Cleanup(func() { _ = stop() })
		addr := listener.Addr().String()

		// First contact: no Cookie header sent, so the proxy must issue one.
		firstHeaders := postUpdateCheckWithCookie(t, client, addr, p.AppID(), "cookie-test-machine", "")
		issued := firstHeaders.Get("Set-Cookie")
		if issued == "" {
			t.Fatal("expected the first cookie-less request to receive a Set-Cookie")
		}
		nameValue := strings.SplitN(issued, ";", 2)[0]
		if nameValue != p.AffinityCookie() {
			t.Fatalf("Set-Cookie name=value %q does not match AffinityCookie() %q", nameValue, p.AffinityCookie())
		}

		// Echoing the issued cookie back must not trigger a second Set-Cookie
		// (real Nebraska's affinity cookie is only (re)issued when the
		// request doesn't already carry one).
		secondHeaders := postUpdateCheckWithCookie(t, client, addr, p.AppID(), "cookie-test-machine", nameValue)
		if got := secondHeaders.Get("Set-Cookie"); got != "" {
			t.Fatalf("expected no Set-Cookie once the cookie is echoed back, got %q", got)
		}

		if got := p.RequestsMissingCookie(); got != 1 {
			t.Fatalf("RequestsMissingCookie() = %d, want 1", got)
		}
		if got := p.RequestsWithCorrectCookie(); got != 1 {
			t.Fatalf("RequestsWithCorrectCookie() = %d, want 1", got)
		}
		if got := p.RequestsWithWrongCookie(); got != 0 {
			t.Fatalf("RequestsWithWrongCookie() = %d, want 0 before any stale cookie is sent", got)
		}

		// A stale/wrong cookie must be classified as such, not silently
		// treated as correct.
		postUpdateCheckWithCookie(t, client, addr, p.AppID(), "cookie-test-machine", "ASLBSA=not-the-real-one")
		if got := p.RequestsWithWrongCookie(); got != 1 {
			t.Fatalf("RequestsWithWrongCookie() = %d, want 1 after sending a stale cookie", got)
		}
	})
}
