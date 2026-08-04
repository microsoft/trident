package proxies

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const RebootStateAnnotation = "trident-acl-agent/reboot-state"

type KubeletProxy struct {
	HTTPClient      *http.Client
	APIServerURL    string
	NodeName        string
	NodeStore       *NodeStore
	BootstrapLabels map[string]string
	MarkerFile      string
	RebootDuration  time.Duration
}

func (k *KubeletProxy) Run(ctx context.Context) error {
	if k.NodeStore != nil {
		if len(k.BootstrapLabels) > 0 {
			k.NodeStore.PatchLabels(k.BootstrapLabels)
		}
		k.NodeStore.SetReadyCondition(true)
	} else {
		if len(k.BootstrapLabels) > 0 {
			if err := patchStringMap(ctx, k.client(), k.nodeURL(), "labels", k.BootstrapLabels); err != nil {
				return err
			}
		}
		if err := patchReadyCondition(ctx, k.client(), k.nodeURL(), true); err != nil {
			return err
		}
	}

	if k.MarkerFile == "" {
		k.MarkerFile = DefaultMarkerFile
	}
	if k.RebootDuration <= 0 {
		k.RebootDuration = 30 * time.Second
	}

	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
			if _, err := os.Stat(k.MarkerFile); err == nil {
				if k.NodeStore != nil {
					k.NodeStore.SetReadyCondition(false)
					k.NodeStore.PatchAnnotations(map[string]string{RebootStateAnnotation: "not-ready"})
				} else {
					if err := patchReadyCondition(ctx, k.client(), k.nodeURL(), false); err != nil {
						return err
					}
					if err := patchStringMap(ctx, k.client(), k.nodeURL(), "annotations", map[string]string{RebootStateAnnotation: "not-ready"}); err != nil {
						return err
					}
				}
				select {
				case <-ctx.Done():
					return ctx.Err()
				case <-time.After(k.RebootDuration):
				}
				if k.NodeStore != nil {
					k.NodeStore.SetReadyCondition(true)
					k.NodeStore.PatchAnnotations(map[string]string{RebootStateAnnotation: "ready"})
				} else {
					if err := patchReadyCondition(ctx, k.client(), k.nodeURL(), true); err != nil {
						return err
					}
					if err := patchStringMap(ctx, k.client(), k.nodeURL(), "annotations", map[string]string{RebootStateAnnotation: "ready"}); err != nil {
						return err
					}
				}
				if err := os.Remove(k.MarkerFile); err != nil && !os.IsNotExist(err) {
					return fmt.Errorf("failed to remove reboot marker %s: %w", k.MarkerFile, err)
				}
			}
		}
	}
}

func WriteRebootMarker(markerFile string) error {
	if markerFile == "" {
		markerFile = DefaultMarkerFile
	}
	if err := os.MkdirAll(filepath.Dir(markerFile), 0o755); err != nil {
		return fmt.Errorf("failed to create reboot marker directory: %w", err)
	}
	return os.WriteFile(markerFile, []byte("reboot-requested\n"), 0o644)
}

func (k *KubeletProxy) nodeURL() string {
	return strings.TrimRight(k.APIServerURL, "/") + "/api/v1/nodes/" + k.NodeName
}

func (k *KubeletProxy) client() *http.Client {
	if k.HTTPClient != nil {
		return k.HTTPClient
	}
	return http.DefaultClient
}

func patchStringMap(ctx context.Context, client *http.Client, nodeURL string, field string, values map[string]string) error {
	body, err := json.Marshal(map[string]any{
		"metadata": map[string]any{field: values},
	})
	if err != nil {
		return err
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPatch, nodeURL, bytes.NewReader(body))
	if err != nil {
		return err
	}
	request.Header.Set("Content-Type", "application/merge-patch+json")
	response, err := client.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 {
		return fmt.Errorf("fake apiserver patch failed with %s", response.Status)
	}
	return nil
}

func patchReadyCondition(ctx context.Context, client *http.Client, nodeURL string, ready bool) error {
	status := corev1.ConditionFalse
	message := "Simulated reboot in progress"
	reason := "TridentACLAgentTesterReboot"
	if ready {
		status = corev1.ConditionTrue
		message = "Node ready"
		reason = "TridentACLAgentTesterReady"
	}

	condition := corev1.NodeCondition{
		Type:               corev1.NodeReady,
		Status:             status,
		LastHeartbeatTime:  metav1.Now(),
		LastTransitionTime: metav1.Now(),
		Reason:             reason,
		Message:            message,
	}

	body, err := json.Marshal(map[string]any{
		"status": map[string]any{
			"conditions": []corev1.NodeCondition{condition},
		},
	})
	if err != nil {
		return err
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPatch, nodeURL, bytes.NewReader(body))
	if err != nil {
		return err
	}
	request.Header.Set("Content-Type", "application/merge-patch+json")
	response, err := client.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 {
		return fmt.Errorf("fake apiserver status patch failed with %s", response.Status)
	}
	return nil
}
