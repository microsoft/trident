package acltester

import (
	"context"
	"encoding/json"
	"net/http/httptest"
	"testing"
	"time"
)

func TestScenarioRunnerObservesExpectedState(t *testing.T) {
	store := NewNodeStore(NewSeedNode("node-a", map[string]string{}))
	server := httptest.NewServer(NewAPIServer("node-a", store).Handler())
	defer server.Close()

	go func() {
		time.Sleep(150 * time.Millisecond)
		store.PatchLabels(map[string]string{
			StateLabel:             "staged",
			ObservedRequestIDLabel: "R1",
		})
	}()

	runner := &RPClient{APIServerURL: server.URL, NodeName: "node-a"}
	scenario := &Scenario{Steps: []ScenarioStep{
		{Patch: &PatchStep{Request: "stage", RequestID: "R1", TargetOSImageVersion: "202507.28.0"}},
		{Expect: &ExpectStep{State: "staged", ObservedRequestID: "R1", Timeout: 2 * time.Second}},
	}}

	report, err := runner.RunScenario(context.Background(), scenario)
	if err != nil {
		t.Fatalf("scenario failed: %v", err)
	}
	if !report.Passed {
		payload, _ := json.Marshal(report)
		t.Fatalf("expected report to pass, got %s", payload)
	}
}

func TestScenarioRunnerChecksFailureReason(t *testing.T) {
	store := NewNodeStore(NewSeedNode("node-b", map[string]string{FailureReasonLabel: "version-mismatch"}))
	server := httptest.NewServer(NewAPIServer("node-b", store).Handler())
	defer server.Close()

	runner := &RPClient{APIServerURL: server.URL, NodeName: "node-b"}
	scenario := &Scenario{Steps: []ScenarioStep{{AssertFailureReason: "version-mismatch"}}}

	report, err := runner.RunScenario(context.Background(), scenario)
	if err != nil {
		t.Fatalf("scenario failed: %v", err)
	}
	if !report.Passed {
		payload, _ := json.Marshal(report)
		t.Fatalf("expected report to pass, got %s", payload)
	}
}
