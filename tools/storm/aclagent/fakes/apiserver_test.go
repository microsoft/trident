package fakes

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	corev1 "k8s.io/api/core/v1"
)

func TestAPIServerGetAndPatchMerge(t *testing.T) {
	store := NewNodeStore(NewSeedNode("node-a", map[string]string{"existing": "true"}))
	server := httptest.NewServer(NewAPIServer("node-a", store).Handler())
	defer server.Close()

	response, err := http.Get(server.URL + "/api/v1/nodes/node-a")
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	defer response.Body.Close()

	var node corev1.Node
	if err := json.NewDecoder(response.Body).Decode(&node); err != nil {
		t.Fatalf("decode failed: %v", err)
	}
	if got := node.Labels["existing"]; got != "true" {
		t.Fatalf("expected existing label, got %q", got)
	}

	patch := map[string]any{
		"metadata": map[string]any{
			"labels":      map[string]any{"existing": nil, "state": "staged"},
			"annotations": map[string]any{"detail": "ok"},
		},
		"status": map[string]any{"conditions": []map[string]any{{"type": "Ready", "status": "False", "reason": "Testing"}}},
	}
	body, err := json.Marshal(patch)
	if err != nil {
		t.Fatalf("marshal failed: %v", err)
	}
	request, err := http.NewRequest(http.MethodPatch, server.URL+"/api/v1/nodes/node-a", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("new request failed: %v", err)
	}
	request.Header.Set("Content-Type", "application/merge-patch+json")
	response, err = http.DefaultClient.Do(request)
	if err != nil {
		t.Fatalf("patch failed: %v", err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d", response.StatusCode)
	}

	node = *store.Snapshot()
	if _, ok := node.Labels["existing"]; ok {
		t.Fatalf("expected existing label removal, labels=%v", node.Labels)
	}
	if got := node.Labels["state"]; got != "staged" {
		t.Fatalf("expected state label staged, got %q", got)
	}
	if got := node.Annotations["detail"]; got != "ok" {
		t.Fatalf("expected detail annotation ok, got %q", got)
	}
	if len(node.Status.Conditions) != 1 || node.Status.Conditions[0].Type != corev1.NodeReady || node.Status.Conditions[0].Status != corev1.ConditionFalse {
		t.Fatalf("expected Ready=False condition, got %#v", node.Status.Conditions)
	}
}
