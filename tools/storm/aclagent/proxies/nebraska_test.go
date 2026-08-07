package proxies

import (
	"context"
	"encoding/xml"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// postOmaha sends a raw Omaha-XML request body to the handler and parses the
// response into an omahaResponse for assertions.
func postOmaha(t *testing.T, handler http.Handler, body string) omahaResponse {
	t.Helper()
	server := httptest.NewServer(handler)
	defer server.Close()

	resp, err := http.Post(server.URL, "application/xml", strings.NewReader(body))
	if err != nil {
		t.Fatalf("post failed: %v", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	var parsed omahaResponse
	if err := xml.NewDecoder(resp.Body).Decode(&parsed); err != nil {
		t.Fatalf("failed to parse response: %v", err)
	}
	return parsed
}

func updateCheckRequest(appID, machineID string) string {
	return `<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0"><app appid="` + appID + `" version="1.0.0" track="stable" machineid="` + machineID + `"><updatecheck/></app></request>`
}

func progressEventRequest(appID, machineID string, eventType, eventResult int) string {
	return `<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0"><app appid="` + appID + `" version="1.0.0" track="stable" machineid="` + machineID + `">` +
		eventXML(eventType, eventResult) + `</app></request>`
}

func batchedCompletionRequest(appID, machineID, previousVersion string, eventType, eventResult int) string {
	return `<?xml version="1.0" encoding="UTF-8"?>
<request protocol="3.0"><app appid="` + appID + `" version="1.0.0" track="stable" machineid="` + machineID + `" previousversion="` + previousVersion + `">` +
		eventXML(eventType, eventResult) + `<ping active="1"/><updatecheck/></app></request>`
}

func eventXML(eventType, eventResult int) string {
	return `<event eventtype="` + itoa(eventType) + `" eventresult="` + itoa(eventResult) + `"/>`
}

func itoa(n int) string {
	// Avoids pulling in strconv just for this - test file, keep it trivial.
	if n == 0 {
		return "0"
	}
	digits := ""
	for n > 0 {
		digits = string(rune('0'+n%10)) + digits
		n /= 10
	}
	return digits
}

func TestNebraskaProxy_NoUpdateWhenScenarioUnavailable(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: false}}

	resp := postOmaha(t, proxy.Handler(), updateCheckRequest("test-app", "machine-1"))

	if len(resp.Apps) != 1 {
		t.Fatalf("expected 1 app in response, got %d", len(resp.Apps))
	}
	app := resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "noupdate" {
		t.Fatalf("expected noupdate, got %+v", app.UpdateCheck)
	}
}

func TestNebraskaProxy_OffersUpdateWhenAvailable(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{
		Available:   true,
		Version:     "2.0.0",
		URL:         "http://example.test/images/",
		SHA384:      "deadbeef",
		PackageName: "acl.cosi",
	}}

	resp := postOmaha(t, proxy.Handler(), updateCheckRequest("test-app", "machine-1"))

	app := resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "ok" {
		t.Fatalf("expected ok, got %+v", app.UpdateCheck)
	}
	if app.UpdateCheck.Manifest == nil || app.UpdateCheck.Manifest.Version != "2.0.0" {
		t.Fatalf("expected manifest version 2.0.0, got %+v", app.UpdateCheck.Manifest)
	}
	pkg := app.UpdateCheck.Manifest.Packages.Entries[0]
	if pkg.Hash != "deadbeef" || pkg.Name != "acl.cosi" {
		t.Fatalf("unexpected package entry: %+v", pkg)
	}
}

func TestNebraskaProxy_EventIsAcknowledged(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: true}}

	resp := postOmaha(t, proxy.Handler(), progressEventRequest("test-app", "machine-1", 13, 1))

	app := resp.Apps[0]
	if len(app.Events) != 1 || app.Events[0].Status != "ok" {
		t.Fatalf("expected a single acknowledged event, got %+v", app.Events)
	}
}

func TestNebraskaProxy_ProgressEventMarksInstanceInProgress(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: true, Version: "2.0.0"}}
	handler := proxy.Handler()

	// DownloadStarted (13,1): commits the instance to in-progress.
	postOmaha(t, handler, progressEventRequest("test-app", "machine-1", 13, 1))

	// A later update check for the same machineid must report
	// error-updateInProgressOnInstance, not the normal offer - this is the
	// specific "expected, not fatal" Nebraska behaviour trident-acl-agent's
	// nebraska client models via CheckOutcome::UpdateInProgress.
	resp := postOmaha(t, handler, updateCheckRequest("test-app", "machine-1"))
	app := resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "error-updateInProgressOnInstance" {
		t.Fatalf("expected error-updateInProgressOnInstance, got %+v", app.UpdateCheck)
	}
}

func TestNebraskaProxy_TerminalCompleteEventClearsInProgress(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: true, Version: "2.0.0"}}
	handler := proxy.Handler()

	postOmaha(t, handler, progressEventRequest("test-app", "machine-1", 800, 1))            // Installed
	postOmaha(t, handler, batchedCompletionRequest("test-app", "machine-1", "1.0.0", 3, 2)) // Completed

	// Nebraska self-heals to Complete; a fresh update check should now see
	// the normal (post-update) scenario state again, not a stale
	// in-progress status.
	resp := postOmaha(t, handler, updateCheckRequest("test-app", "machine-1"))
	app := resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "ok" {
		t.Fatalf("expected the in-progress wedge to be cleared, got %+v", app.UpdateCheck)
	}
}

func TestNebraskaProxy_TerminalFailedEventClearsInProgress(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: true, Version: "2.0.0"}}
	handler := proxy.Handler()

	postOmaha(t, handler, progressEventRequest("test-app", "machine-1", 13, 1)) // DownloadStarted
	postOmaha(t, handler, progressEventRequest("test-app", "machine-1", 3, 0))  // Failed

	resp := postOmaha(t, handler, updateCheckRequest("test-app", "machine-1"))
	app := resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "ok" {
		t.Fatalf("expected the wedge to be released after a Failed event, got %+v", app.UpdateCheck)
	}
}

func TestNebraskaProxy_InstancesAreIndependentByMachineID(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: true, Version: "2.0.0"}}
	handler := proxy.Handler()

	postOmaha(t, handler, progressEventRequest("test-app", "machine-1", 13, 1))

	// machine-2 never sent a progress event, so its update check must not
	// be affected by machine-1's in-progress state.
	resp := postOmaha(t, handler, updateCheckRequest("test-app", "machine-2"))
	app := resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "ok" {
		t.Fatalf("expected machine-2 to be unaffected by machine-1's state, got %+v", app.UpdateCheck)
	}

	// machine-1 must still be reported as in progress.
	resp = postOmaha(t, handler, updateCheckRequest("test-app", "machine-1"))
	app = resp.Apps[0]
	if app.UpdateCheck == nil || app.UpdateCheck.Status != "error-updateInProgressOnInstance" {
		t.Fatalf("expected machine-1 to remain in progress, got %+v", app.UpdateCheck)
	}
}

func TestNebraskaProxy_ListenAndServeShutsDownOnContextCancel(t *testing.T) {
	proxy := &NebraskaProxy{Scenario: &NebraskaScenario{Available: false}}
	ctx, cancel := context.WithCancel(context.Background())
	listener, err := proxy.ListenAndServe(ctx, "127.0.0.1:0")
	if err != nil {
		t.Fatalf("ListenAndServe failed: %v", err)
	}
	cancel()
	_ = listener.Close()
}
