package proxies

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	corev1 "k8s.io/api/core/v1"
)

// newTestAPIServer spins up an in-process httptest.Server backed by a fresh
// NodeStore/APIServer pair, mirroring how the storm test cases themselves
// construct these (see e.g. tests/node_resilience.go's RunNodeResilience).
func newTestAPIServer(t *testing.T) (*httptest.Server, *NodeStore) {
	t.Helper()
	const nodeName = "test-node"
	store := NewNodeStore(NewSeedNode(nodeName, map[string]string{}, "test-system-uuid"))
	server := NewAPIServer(nodeName, store)
	ts := httptest.NewServer(server.Handler())
	t.Cleanup(ts.Close)
	return ts, store
}

// TestNodeStoreDeleteNodeMakesGetReturn404 confirms the fake apiserver
// starts out serving the seeded Node normally, then returns HTTP 404 for a
// single-object GET once DeleteNode is called, matching the behavior
// trident-acl-agent's k8s client (map_kube_error) turns into NodeGone.
func TestNodeStoreDeleteNodeMakesGetReturn404(t *testing.T) {
	ts, store := newTestAPIServer(t)

	resp, err := http.Get(ts.URL + "/api/v1/nodes/test-node")
	if err != nil {
		t.Fatalf("unexpected error on GET before DeleteNode: %v", err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200 before DeleteNode, got %d", resp.StatusCode)
	}

	store.DeleteNode()

	resp, err = http.Get(ts.URL + "/api/v1/nodes/test-node")
	if err != nil {
		t.Fatalf("unexpected error on GET after DeleteNode: %v", err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("expected 404 after DeleteNode, got %d", resp.StatusCode)
	}
}

// TestNodeStoreDeleteNodeMakesPatchReturn404 confirms a merge-patch PATCH
// against the single-object endpoint also 404s while the Node is "deleted" -
// the call path trident-acl-agent's heartbeat/status publish uses.
func TestNodeStoreDeleteNodeMakesPatchReturn404(t *testing.T) {
	ts, store := newTestAPIServer(t)
	store.DeleteNode()

	req, err := http.NewRequest(http.MethodPatch, ts.URL+"/api/v1/nodes/test-node", nil)
	if err != nil {
		t.Fatalf("failed to build PATCH request: %v", err)
	}
	req.Header.Set("Content-Type", "application/merge-patch+json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("unexpected error on PATCH after DeleteNode: %v", err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("expected 404 after DeleteNode, got %d", resp.StatusCode)
	}
}

// TestNodeStoreDeleteNodeMakesListEmpty confirms the collection LIST
// endpoint returns a well-formed, empty NodeList (never a 404) while the
// Node is "deleted" - matching real Kubernetes' behavior for a
// fieldSelector-filtered LIST matching zero objects, which kube-rs's
// watcher() relies on to never itself observe a 404 here.
func TestNodeStoreDeleteNodeMakesListEmpty(t *testing.T) {
	ts, store := newTestAPIServer(t)
	store.DeleteNode()

	resp, err := http.Get(ts.URL + "/api/v1/nodes?fieldSelector=metadata.name=test-node")
	if err != nil {
		t.Fatalf("unexpected error on LIST after DeleteNode: %v", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200 (empty list) after DeleteNode, got %d", resp.StatusCode)
	}
}

// TestNodeStoreRestoreNodeUndoesDeleteNode confirms RestoreNode brings GET
// back to succeeding, with the Node's prior state intact.
func TestNodeStoreRestoreNodeUndoesDeleteNode(t *testing.T) {
	ts, store := newTestAPIServer(t)
	store.PatchLabels(map[string]string{"example": "value"})
	store.DeleteNode()
	store.RestoreNode()

	resp, err := http.Get(ts.URL + "/api/v1/nodes/test-node")
	if err != nil {
		t.Fatalf("unexpected error on GET after RestoreNode: %v", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200 after RestoreNode, got %d", resp.StatusCode)
	}

	snapshot := store.Snapshot()
	if snapshot.Labels["example"] != "value" {
		t.Fatalf("expected label set before DeleteNode to survive the delete/restore cycle, got %+v", snapshot.Labels)
	}
}

// TestNodeStoreSeedsSystemUUID confirms a GET serves the seeded
// status.nodeInfo.systemUUID verbatim - the field trident-acl-agent's
// NodeClient::get_node (crates/trident-acl-agent/src/annotations/k8s.rs)
// compares against the VM's own /sys/class/dmi/id/product_uuid.
func TestNodeStoreSeedsSystemUUID(t *testing.T) {
	ts, _ := newTestAPIServer(t)

	resp, err := http.Get(ts.URL + "/api/v1/nodes/test-node")
	if err != nil {
		t.Fatalf("unexpected error on GET: %v", err)
	}
	defer resp.Body.Close()

	var node corev1.Node
	if err := json.NewDecoder(resp.Body).Decode(&node); err != nil {
		t.Fatalf("failed to decode Node response: %v", err)
	}
	if node.Status.NodeInfo.SystemUUID != "test-system-uuid" {
		t.Fatalf("expected systemUUID %q, got %q", "test-system-uuid", node.Status.NodeInfo.SystemUUID)
	}
}

// TestNodeStoreSetSystemUUID confirms SetSystemUUID is reflected on the
// very next GET - run-node-resilience's phase 3 uses this to simulate a
// Node that GETs successfully but whose systemUUID doesn't match the VM's
// hardware, which NodeClient::get_node must treat the same as NodeGone.
func TestNodeStoreSetSystemUUID(t *testing.T) {
	ts, store := newTestAPIServer(t)
	store.SetSystemUUID("mismatched-uuid")

	resp, err := http.Get(ts.URL + "/api/v1/nodes/test-node")
	if err != nil {
		t.Fatalf("unexpected error on GET after SetSystemUUID: %v", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200 after SetSystemUUID (the Node itself is not gone), got %d", resp.StatusCode)
	}

	var node corev1.Node
	if err := json.NewDecoder(resp.Body).Decode(&node); err != nil {
		t.Fatalf("failed to decode Node response: %v", err)
	}
	if node.Status.NodeInfo.SystemUUID != "mismatched-uuid" {
		t.Fatalf("expected systemUUID %q, got %q", "mismatched-uuid", node.Status.NodeInfo.SystemUUID)
	}
}
