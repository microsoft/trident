package proxies

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"strings"
	"sync"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
)

type NodeStore struct {
	mu              sync.RWMutex
	node            *corev1.Node
	watchers        map[int]chan *corev1.Node
	nextID          int
	resourceVersion int64
}

func NewSeedNode(name string, labels map[string]string) *corev1.Node {
	seed := &corev1.Node{
		TypeMeta: metav1.TypeMeta{APIVersion: "v1", Kind: "Node"},
		ObjectMeta: metav1.ObjectMeta{
			Name:        name,
			Labels:      map[string]string{},
			Annotations: map[string]string{},
		},
	}
	for key, value := range labels {
		seed.Labels[key] = value
	}
	return seed
}

func LoadSeedNode(data []byte) (*corev1.Node, error) {
	var node corev1.Node
	if err := json.Unmarshal(data, &node); err != nil {
		return nil, fmt.Errorf("failed to parse seed node json: %w", err)
	}
	if node.Name == "" {
		return nil, fmt.Errorf("seed node json must set metadata.name")
	}
	if node.APIVersion == "" {
		node.APIVersion = "v1"
	}
	if node.Kind == "" {
		node.Kind = "Node"
	}
	if node.Labels == nil {
		node.Labels = map[string]string{}
	}
	if node.Annotations == nil {
		node.Annotations = map[string]string{}
	}
	return &node, nil
}

func NewNodeStore(seed *corev1.Node) *NodeStore {
	node := seed.DeepCopy()
	store := &NodeStore{node: node, watchers: map[int]chan *corev1.Node{}, resourceVersion: 1}
	node.ResourceVersion = "1"
	return store
}

// bumpLocked increments the store's resourceVersion counter and stamps it
// onto the current node object. Every real Kubernetes object always carries
// metadata.resourceVersion, and kube-rs's watcher() rejects watch events
// (and the LIST used to bootstrap a watch) that omit it, so this must be
// set on every mutation. Callers must hold s.mu for writing.
func (s *NodeStore) bumpLocked() {
	s.resourceVersion++
	s.node.ResourceVersion = fmt.Sprintf("%d", s.resourceVersion)
}

func (s *NodeStore) Snapshot() *corev1.Node {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.node.DeepCopy()
}

func (s *NodeStore) MergePatch(raw []byte) (*corev1.Node, error) {
	var patch metadataPatch
	if err := json.Unmarshal(raw, &patch); err != nil {
		return nil, fmt.Errorf("failed to parse merge patch: %w", err)
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	applyOptionalStringMap(s.node.Labels, patch.Metadata.Labels)
	applyOptionalStringMap(s.node.Annotations, patch.Metadata.Annotations)
	if patch.Status.Conditions != nil {
		s.node.Status.Conditions = append([]corev1.NodeCondition(nil), (*patch.Status.Conditions)...)
	}
	s.bumpLocked()
	s.broadcastLocked()
	return s.node.DeepCopy(), nil
}

func (s *NodeStore) PatchLabels(labels map[string]string) *corev1.Node {
	s.mu.Lock()
	defer s.mu.Unlock()
	for key, value := range labels {
		s.node.Labels[key] = value
	}
	s.bumpLocked()
	s.broadcastLocked()
	return s.node.DeepCopy()
}

func (s *NodeStore) PatchAnnotations(annotations map[string]string) *corev1.Node {
	s.mu.Lock()
	defer s.mu.Unlock()
	for key, value := range annotations {
		s.node.Annotations[key] = value
	}
	s.bumpLocked()
	s.broadcastLocked()
	return s.node.DeepCopy()
}

func (s *NodeStore) SetReadyCondition(ready bool) *corev1.Node {
	s.mu.Lock()
	defer s.mu.Unlock()
	status := corev1.ConditionFalse
	message := "Simulated reboot in progress"
	reason := "TridentAKSAgentTesterReboot"
	if ready {
		status = corev1.ConditionTrue
		message = "Node ready"
		reason = "TridentAKSAgentTesterReady"
	}
	s.node.Status.Conditions = []corev1.NodeCondition{{
		Type:               corev1.NodeReady,
		Status:             status,
		LastHeartbeatTime:  metav1.Now(),
		LastTransitionTime: metav1.Now(),
		Reason:             reason,
		Message:            message,
	}}
	s.bumpLocked()
	s.broadcastLocked()
	return s.node.DeepCopy()
}

func (s *NodeStore) Subscribe() (int, <-chan *corev1.Node, *corev1.Node) {
	s.mu.Lock()
	defer s.mu.Unlock()
	id := s.nextID
	s.nextID++
	ch := make(chan *corev1.Node, 8)
	s.watchers[id] = ch
	return id, ch, s.node.DeepCopy()
}

func (s *NodeStore) Unsubscribe(id int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if ch, ok := s.watchers[id]; ok {
		delete(s.watchers, id)
		close(ch)
	}
}

func (s *NodeStore) broadcastLocked() {
	snapshot := s.node.DeepCopy()
	for _, ch := range s.watchers {
		select {
		case ch <- snapshot.DeepCopy():
		default:
		}
	}
}

type APIServer struct {
	nodeName string
	store    *NodeStore
}

func NewAPIServer(nodeName string, store *NodeStore) *APIServer {
	return &APIServer{nodeName: nodeName, store: store}
}

func (s *APIServer) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/v1/nodes":
			// Collection endpoint. kube-rs's watcher always performs an
			// initial LIST here (optionally filtered by fieldSelector) before
			// switching to a watch on the same collection; both must be
			// served or the watcher treats the 404 as fatal and the process
			// exits, taking down the whole service.
			if r.Method != http.MethodGet {
				http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
				return
			}
			if r.URL.Query().Get("watch") == "true" {
				s.handleWatch(w, r)
				return
			}
			s.handleList(w, r)
			return
		case "/api/v1/nodes/" + s.nodeName:
			if r.Method == http.MethodGet && r.URL.Query().Get("watch") == "true" {
				s.handleWatch(w, r)
				return
			}
			switch r.Method {
			case http.MethodGet:
				s.handleGet(w, r)
			case http.MethodPatch:
				s.handlePatch(w, r)
			default:
				http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			}
			return
		default:
			http.NotFound(w, r)
		}
	})
}

func (s *APIServer) ListenAndServe(ctx context.Context, listenAddr string) (net.Listener, error) {
	listener, err := net.Listen("tcp", listenAddr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on %s: %w", listenAddr, err)
	}
	server := &http.Server{Handler: s.Handler()}
	go func() {
		<-ctx.Done()
		_ = server.Shutdown(context.Background())
	}()
	go func() { _ = server.Serve(listener) }()
	return listener, nil
}

func (s *APIServer) handleGet(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, s.store.Snapshot())
}

// handleList serves the collection endpoint's plain (non-watch) LIST
// request. kube-rs's watcher() issues this before it ever opens a watch
// stream, so it must return a well-formed NodeList (including
// metadata.resourceVersion) even though this fake only ever tracks one node.
func (s *APIServer) handleList(w http.ResponseWriter, r *http.Request) {
	node := s.store.Snapshot()
	items := []corev1.Node{}
	if selector := r.URL.Query().Get("fieldSelector"); selector != "" {
		if selector == "metadata.name="+s.nodeName {
			items = append(items, *node)
		}
	} else {
		items = append(items, *node)
	}
	list := corev1.NodeList{
		TypeMeta: metav1.TypeMeta{APIVersion: "v1", Kind: "NodeList"},
		ListMeta: metav1.ListMeta{ResourceVersion: node.ResourceVersion},
		Items:    items,
	}
	writeJSON(w, http.StatusOK, &list)
}

func (s *APIServer) handlePatch(w http.ResponseWriter, r *http.Request) {
	if contentType := r.Header.Get("Content-Type"); contentType != "" && !strings.Contains(contentType, "merge-patch+json") {
		http.Error(w, "expected application/merge-patch+json", http.StatusUnsupportedMediaType)
		return
	}
	defer r.Body.Close()
	body := json.NewDecoder(r.Body)
	body.DisallowUnknownFields()
	var raw map[string]any
	if err := body.Decode(&raw); err != nil {
		http.Error(w, fmt.Sprintf("invalid patch body: %v", err), http.StatusBadRequest)
		return
	}
	bytes, err := json.Marshal(raw)
	if err != nil {
		http.Error(w, fmt.Sprintf("failed to re-marshal patch: %v", err), http.StatusInternalServerError)
		return
	}
	updated, err := s.store.MergePatch(bytes)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	writeJSON(w, http.StatusOK, updated)
}

func (s *APIServer) handleWatch(w http.ResponseWriter, r *http.Request) {
	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", http.StatusInternalServerError)
		return
	}
	watchID, ch, current := s.store.Subscribe()
	defer s.store.Unsubscribe(watchID)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	if err := writeWatchEvent(w, "ADDED", current); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	flusher.Flush()
	for {
		select {
		case <-r.Context().Done():
			return
		case node, ok := <-ch:
			if !ok {
				return
			}
			if err := writeWatchEvent(w, "MODIFIED", node); err != nil {
				return
			}
			flusher.Flush()
		}
	}
}

func writeWatchEvent(w http.ResponseWriter, eventType string, node *corev1.Node) error {
	raw, err := json.Marshal(node)
	if err != nil {
		return err
	}
	event := metav1.WatchEvent{Type: eventType, Object: runtime.RawExtension{Raw: raw}}
	return json.NewEncoder(w).Encode(&event)
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

type metadataPatch struct {
	Metadata struct {
		Labels      map[string]*string `json:"labels"`
		Annotations map[string]*string `json:"annotations"`
	} `json:"metadata"`
	Status struct {
		Conditions *[]corev1.NodeCondition `json:"conditions"`
	} `json:"status"`
}

func applyOptionalStringMap(target map[string]string, patch map[string]*string) {
	for key, value := range patch {
		if value == nil {
			delete(target, key)
			continue
		}
		target[key] = *value
	}
}
